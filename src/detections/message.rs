extern crate lazy_static;
use crate::detections::configs;
use crate::detections::configs::CURRENT_EXE_PATH;
use crate::detections::utils;
use crate::detections::utils::get_serde_number_to_string;
use crate::detections::utils::write_color_buffer;
use crate::options::profile::PROFILES;
use chrono::{DateTime, Local, Utc};
use dashmap::DashMap;
use hashbrown::HashMap;
use lazy_static::lazy_static;
use linked_hash_map::LinkedHashMap;
use regex::Regex;
use serde_json::Value;
use std::env;
use std::fs::create_dir;
use std::fs::File;
use std::io::BufWriter;
use std::io::{self, Write};
use std::path::Path;
use std::sync::Mutex;
use termcolor::{BufferWriter, ColorChoice};

#[derive(Debug, Clone)]
pub struct DetectInfo {
    pub rulepath: String,
    pub ruletitle: String,
    pub level: String,
    pub computername: String,
    pub eventid: String,
    pub detail: String,
    pub record_information: Option<String>,
    pub ext_field: LinkedHashMap<String, String>,
}

pub struct AlertMessage {}

lazy_static! {
    #[derive(Debug,PartialEq, Eq, Ord, PartialOrd)]
    pub static ref MESSAGES: DashMap<DateTime<Utc>, Vec<DetectInfo>> = DashMap::new();
    pub static ref ALIASREGEX: Regex = Regex::new(r"%[a-zA-Z0-9-_\[\]]+%").unwrap();
    pub static ref SUFFIXREGEX: Regex = Regex::new(r"\[([0-9]+)\]").unwrap();
    pub static ref ERROR_LOG_PATH: String = format!(
        "./logs/errorlog-{}.log",
        Local::now().format("%Y%m%d_%H%M%S")
    );
    pub static ref QUIET_ERRORS_FLAG: bool = configs::CONFIG.read().unwrap().args.quiet_errors;
    pub static ref ERROR_LOG_STACK: Mutex<Vec<String>> = Mutex::new(Vec::new());
    pub static ref STATISTICS_FLAG: bool = configs::CONFIG.read().unwrap().args.statistics;
    pub static ref LOGONSUMMARY_FLAG: bool = configs::CONFIG.read().unwrap().args.logon_summary;
    pub static ref TAGS_CONFIG: HashMap<String, String> = create_output_filter_config(
        utils::check_setting_path(&CURRENT_EXE_PATH.to_path_buf(), "config/mitre_tactics.txt", true)
            .unwrap().to_str()
            .unwrap(),
    );
    pub static ref CH_CONFIG: HashMap<String, String> = create_output_filter_config(
        utils::check_setting_path(&configs::CONFIG.read().unwrap().args.config, "channel_abbreviations.txt", false).unwrap_or_else(|| {
            utils::check_setting_path(
                &CURRENT_EXE_PATH.to_path_buf(),
                "rules/config/channel_abbreviations.txt", true
            ).unwrap()
            })
        .to_str()
        .unwrap(),
    );
    pub static ref PIVOT_KEYWORD_LIST_FLAG: bool =
        configs::CONFIG.read().unwrap().args.pivot_keywords_list;
    pub static ref DEFAULT_DETAILS: HashMap<String, String> = get_default_details(
        utils::check_setting_path(&configs::CONFIG.read().unwrap().args.config, "default_details.txt", false).unwrap_or_else(|| {
            utils::check_setting_path(
                &CURRENT_EXE_PATH.to_path_buf(),
                "rules/config/default_details.txt", true
            ).unwrap()
        })
        .to_str()
        .unwrap()
    );
    pub static ref LEVEL_ABBR: LinkedHashMap<String, String> = LinkedHashMap::from_iter([
        ("critical".to_string(), "crit".to_string()),
        ("high".to_string(), "high".to_string()),
        ("medium".to_string(), "med ".to_string()),
        ("low".to_string(), "low ".to_string()),
        ("informational".to_string(), "info".to_string()),
    ]);
    pub static ref LEVEL_FULL: HashMap<String, String> = HashMap::from([
        ("crit".to_string(), "critical".to_string()),
        ("high".to_string(), "high".to_string()),
        ("med ".to_string(), "medium".to_string()),
        ("low ".to_string(), "low".to_string()),
        ("info".to_string(), "informational".to_string())
    ]);
}

/// ファイルパスで記載されたtagでのフル名、表示の際に置き換えられる文字列のHashMapを作成する関数。
/// ex. attack.impact,Impact
pub fn create_output_filter_config(path: &str) -> HashMap<String, String> {
    let mut ret: HashMap<String, String> = HashMap::new();
    let read_result = utils::read_csv(path);
    if read_result.is_err() {
        AlertMessage::alert(read_result.as_ref().unwrap_err()).ok();
        return HashMap::default();
    }
    read_result.unwrap().into_iter().for_each(|line| {
        if line.len() != 2 {
            return;
        }

        let tag_full_str = line[0].trim().to_ascii_lowercase();
        let tag_replace_str = line[1].trim();

        ret.insert(tag_full_str, tag_replace_str.to_owned());
    });
    ret
}

/// メッセージの設定を行う関数。aggcondition対応のためrecordではなく出力をする対象時間がDatetime形式での入力としている
pub fn insert_message(detect_info: DetectInfo, event_time: DateTime<Utc>) {
    let mut v = MESSAGES.entry(event_time).or_default();
    let (_, info) = v.pair_mut();
    info.push(detect_info);
}

/// メッセージを設定
pub fn insert(
    event_record: &Value,
    output: String,
    mut detect_info: DetectInfo,
    time: DateTime<Utc>,
    profile_converter: &mut HashMap<String, String>,
    is_agg: bool,
) {
    if !is_agg {
        let parsed_detail = parse_message(event_record, &output)
            .chars()
            .filter(|&c| !c.is_control())
            .collect::<String>();
        detect_info.detail = if parsed_detail.is_empty() {
            "-".to_string()
        } else {
            parsed_detail
        };
    }
    let mut exist_detail = false;
    PROFILES.as_ref().unwrap().iter().for_each(|(_k, v)| {
        if v.contains("%Details%") {
            exist_detail = true;
        }
    });
    if exist_detail {
        profile_converter.insert("%Details%".to_string(), detect_info.detail.to_owned());
    }
    let mut tmp_converted_info: LinkedHashMap<String, String> = LinkedHashMap::new();
    for (k, v) in &detect_info.ext_field {
        let converted_reserve_info = convert_profile_reserved_info(v, profile_converter);
        if v.contains("%RecordInformation%") || v.contains("%Details%") {
            tmp_converted_info.insert(k.to_owned(), converted_reserve_info);
        } else {
            tmp_converted_info.insert(
                k.to_owned(),
                parse_message(event_record, &converted_reserve_info),
            );
        }
    }
    for (k, v) in tmp_converted_info {
        detect_info.ext_field.insert(k, v);
    }
    insert_message(detect_info, time)
}

/// profileで用いられる予約語の情報を変換する関数
fn convert_profile_reserved_info(
    output: &String,
    config_reserved_info: &HashMap<String, String>,
) -> String {
    let mut ret = output.to_owned();
    config_reserved_info.iter().for_each(|(k, v)| {
        ret = ret.replace(k, v);
    });
    ret
}

/// メッセージ内の%で囲まれた箇所をエイリアスとしてをレコード情報を参照して置き換える関数
fn parse_message(event_record: &Value, output: &String) -> String {
    let mut return_message = output.to_owned();
    let mut hash_map: HashMap<String, String> = HashMap::new();
    for caps in ALIASREGEX.captures_iter(&return_message) {
        let full_target_str = &caps[0];
        let target_length = full_target_str.chars().count() - 2; // The meaning of 2 is two percent
        let target_str = full_target_str
            .chars()
            .skip(1)
            .take(target_length)
            .collect::<String>();

        let array_str = if let Some(_array_str) = configs::EVENTKEY_ALIAS.get_event_key(&target_str)
        {
            _array_str.to_string()
        } else {
            format!("Event.EventData.{}", target_str)
        };

        let split: Vec<&str> = array_str.split('.').collect();
        let mut tmp_event_record: &Value = event_record;
        for s in &split {
            if let Some(record) = tmp_event_record.get(s) {
                tmp_event_record = record;
            }
        }
        let suffix_match = SUFFIXREGEX.captures(&target_str);
        let suffix: i64 = match suffix_match {
            Some(cap) => cap.get(1).map_or(-1, |a| a.as_str().parse().unwrap_or(-1)),
            None => -1,
        };
        if suffix >= 1 {
            tmp_event_record = tmp_event_record
                .get("Data")
                .unwrap()
                .get((suffix - 1) as usize)
                .unwrap_or(tmp_event_record);
        }
        let hash_value = get_serde_number_to_string(tmp_event_record);
        if hash_value.is_some() {
            if let Some(hash_value) = hash_value {
                // UnicodeのWhitespace characterをそのままCSVに出力すると見難いので、スペースに変換する。なお、先頭と最後のWhitespace characterは単に削除される。
                let hash_value: Vec<&str> = hash_value.split_whitespace().collect();
                let hash_value = hash_value.join(" ");
                hash_map.insert(full_target_str.to_string(), hash_value);
            }
        } else {
            hash_map.insert(full_target_str.to_string(), "n/a".to_string());
        }
    }

    for (k, v) in &hash_map {
        return_message = return_message.replace(k, v);
    }
    return_message
}

/// メッセージを返す
pub fn get(time: DateTime<Utc>) -> Vec<DetectInfo> {
    match MESSAGES.get(&time) {
        Some(v) => v.to_vec(),
        None => Vec::new(),
    }
}

pub fn get_event_time(event_record: &Value) -> Option<DateTime<Utc>> {
    let system_time = &event_record["Event"]["System"]["TimeCreated_attributes"]["SystemTime"];
    return utils::str_time_to_datetime(system_time.as_str().unwrap_or(""));
}

/// detailsのdefault値をファイルから読み取る関数
pub fn get_default_details(filepath: &str) -> HashMap<String, String> {
    let read_result = utils::read_csv(filepath);
    match read_result {
        Err(_e) => {
            AlertMessage::alert(&_e).ok();
            HashMap::new()
        }
        Ok(lines) => {
            let mut ret: HashMap<String, String> = HashMap::new();
            lines
                .into_iter()
                .try_for_each(|line| -> Result<(), String> {
                    let provider = match line.get(0) {
                        Some(_provider) => _provider.trim(),
                        _ => {
                            return Result::Err(
                                "Failed to read provider in default_details.txt.".to_string(),
                            )
                        }
                    };
                    let eid = match line.get(1) {
                        Some(eid_str) => match eid_str.trim().parse::<i64>() {
                            Ok(_eid) => _eid,
                            _ => {
                                return Result::Err(
                                    "Parse Error EventID in default_details.txt.".to_string(),
                                )
                            }
                        },
                        _ => {
                            return Result::Err(
                                "Failed to read EventID in default_details.txt.".to_string(),
                            )
                        }
                    };
                    let details = match line.get(2) {
                        Some(detail) => detail.trim(),
                        _ => {
                            return Result::Err(
                                "Failed to read details in default_details.txt.".to_string(),
                            )
                        }
                    };
                    ret.insert(format!("{}_{}", provider, eid), details.to_string());
                    Ok(())
                })
                .ok();
            ret
        }
    }
}

impl AlertMessage {
    ///対象のディレクトリが存在することを確認後、最初の定型文を追加して、ファイルのbufwriterを返す関数
    pub fn create_error_log(path_str: String) {
        if *QUIET_ERRORS_FLAG {
            return;
        }
        let path = Path::new(&path_str);
        if !path.parent().unwrap().exists() {
            create_dir(path.parent().unwrap()).ok();
        }
        let mut error_log_writer = BufWriter::new(File::create(path).unwrap());
        error_log_writer
            .write_all(
                format!(
                    "user input: {:?}\n",
                    format_args!("{}", env::args().collect::<Vec<String>>().join(" "))
                )
                .as_bytes(),
            )
            .ok();
        let error_logs = ERROR_LOG_STACK.lock().unwrap();
        error_logs.iter().for_each(|error_log| {
            writeln!(error_log_writer, "{}", error_log).ok();
        });
        println!(
            "Errors were generated. Please check {} for details.",
            *ERROR_LOG_PATH
        );
        println!();
    }

    /// ERRORメッセージを表示する関数
    pub fn alert(contents: &str) -> io::Result<()> {
        write_color_buffer(
            &BufferWriter::stderr(ColorChoice::Always),
            None,
            &format!("[ERROR] {}", contents),
            true,
        )
    }

    /// WARNメッセージを表示する関数
    pub fn warn(contents: &str) -> io::Result<()> {
        write_color_buffer(
            &BufferWriter::stderr(ColorChoice::Always),
            None,
            &format!("[WARN] {}", contents),
            true,
        )
    }
}

#[cfg(test)]
mod tests {
    use crate::detections::message::{get, insert_message, AlertMessage, DetectInfo};
    use crate::detections::message::{parse_message, MESSAGES};
    use chrono::Utc;
    use hashbrown::HashMap;
    use rand::Rng;
    use serde_json::Value;
    use std::thread;
    use std::time::Duration;

    use super::{create_output_filter_config, get_default_details};

    #[test]
    fn test_error_message() {
        let input = "TEST!";
        AlertMessage::alert(input).expect("[ERROR] TEST!");
    }

    #[test]
    fn test_warn_message() {
        let input = "TESTWarn!";
        AlertMessage::warn(input).expect("[WARN] TESTWarn!");
    }

    #[test]
    /// outputで指定されているキー(eventkey_alias.txt内で設定済み)から対象のレコード内の情報でメッセージをパースしているか確認する関数
    fn test_parse_message() {
        MESSAGES.clear();
        let json_str = r##"
        {
            "Event": {
                "EventData": {
                    "CommandLine": "parsetest1"
                },
                "System": {
                    "Computer": "testcomputer1",
                    "TimeCreated_attributes": {
                        "SystemTime": "1996-02-27T01:05:01Z"
                    }
                }
            }
        }
    "##;
        let event_record: Value = serde_json::from_str(json_str).unwrap();
        let expected = "commandline:parsetest1 computername:testcomputer1";
        assert_eq!(
            parse_message(
                &event_record,
                &"commandline:%CommandLine% computername:%ComputerName%".to_owned()
            ),
            expected,
        );
    }

    #[test]
    fn test_parse_message_auto_search() {
        MESSAGES.clear();
        let json_str = r##"
        {
            "Event": {
                "EventData": {
                    "NoAlias": "no_alias"
                }
            }
        }
    "##;
        let event_record: Value = serde_json::from_str(json_str).unwrap();
        let expected = "alias:no_alias";
        assert_eq!(
            parse_message(&event_record, &"alias:%NoAlias%".to_owned()),
            expected,
        );
    }

    #[test]
    /// outputで指定されているキーが、eventkey_alias.txt内で設定されていない場合の出力テスト
    fn test_parse_message_not_exist_key_in_output() {
        MESSAGES.clear();
        let json_str = r##"
        {
            "Event": {
                "EventData": {
                    "CommandLine": "parsetest2"
                },
                "System": {
                    "TimeCreated_attributes": {
                        "SystemTime": "1996-02-27T01:05:01Z"
                    }
                }
            }
        }
    "##;
        let event_record: Value = serde_json::from_str(json_str).unwrap();
        let expected = "NoExistAlias:n/a";
        assert_eq!(
            parse_message(&event_record, &"NoExistAlias:%NoAliasNoHit%".to_owned()),
            expected,
        );
    }
    #[test]
    /// output test when no exist info in target record output and described key-value data in eventkey_alias.txt
    fn test_parse_message_not_exist_value_in_record() {
        MESSAGES.clear();
        let json_str = r##"
        {
            "Event": {
                "EventData": {
                    "CommandLine": "parsetest3"
                },
                "System": {
                    "TimeCreated_attributes": {
                        "SystemTime": "1996-02-27T01:05:01Z"
                    }
                }
            }
        }
    "##;
        let event_record: Value = serde_json::from_str(json_str).unwrap();
        let expected = "commandline:parsetest3 computername:n/a";
        assert_eq!(
            parse_message(
                &event_record,
                &"commandline:%CommandLine% computername:%ComputerName%".to_owned()
            ),
            expected,
        );
    }
    #[test]
    /// output test when no exist info in target record output and described key-value data in eventkey_alias.txt
    fn test_parse_message_multiple_no_suffix_in_record() {
        MESSAGES.clear();
        let json_str = r##"
        {
            "Event": {
                "EventData": {
                    "CommandLine": "parsetest3",
                    "Data": [
                        "data1", 
                        "data2", 
                        "data3"
                    ]
                },
                "System": {
                    "TimeCreated_attributes": {
                        "SystemTime": "1996-02-27T01:05:01Z"
                    }
                }
            }
        }
    "##;
        let event_record: Value = serde_json::from_str(json_str).unwrap();
        let expected = "commandline:parsetest3 data:[\"data1\",\"data2\",\"data3\"]";
        assert_eq!(
            parse_message(
                &event_record,
                &"commandline:%CommandLine% data:%Data%".to_owned()
            ),
            expected,
        );
    }
    #[test]
    /// output test when no exist info in target record output and described key-value data in eventkey_alias.txt
    fn test_parse_message_multiple_with_suffix_in_record() {
        MESSAGES.clear();
        let json_str = r##"
        {
            "Event": {
                "EventData": {
                    "CommandLine": "parsetest3",
                    "Data": [
                        "data1", 
                        "data2", 
                        "data3"
                    ]
                },
                "System": {
                    "TimeCreated_attributes": {
                        "SystemTime": "1996-02-27T01:05:01Z"
                    }
                }
            }
        }
    "##;
        let event_record: Value = serde_json::from_str(json_str).unwrap();
        let expected = "commandline:parsetest3 data:data2";
        assert_eq!(
            parse_message(
                &event_record,
                &"commandline:%CommandLine% data:%Data[2]%".to_owned()
            ),
            expected,
        );
    }
    #[test]
    /// output test when no exist info in target record output and described key-value data in eventkey_alias.txt
    fn test_parse_message_multiple_no_exist_in_record() {
        MESSAGES.clear();
        let json_str = r##"
        {
            "Event": {
                "EventData": {
                    "CommandLine": "parsetest3",
                    "Data": [
                        "data1", 
                        "data2",
                        "data3"
                    ]
                },
                "System": {
                    "TimeCreated_attributes": {
                        "SystemTime": "1996-02-27T01:05:01Z"
                    }
                }
            }
        }
    "##;
        let event_record: Value = serde_json::from_str(json_str).unwrap();
        let expected = "commandline:parsetest3 data:n/a";
        assert_eq!(
            parse_message(
                &event_record,
                &"commandline:%CommandLine% data:%Data[0]%".to_owned()
            ),
            expected,
        );
    }
    #[test]
    /// test of loading output filter config by mitre_tactics.txt
    fn test_load_mitre_tactics_log() {
        let actual = create_output_filter_config("test_files/config/mitre_tactics.txt");
        let expected: HashMap<String, String> = HashMap::from([
            ("attack.impact".to_string(), "Impact".to_string()),
            ("xxx".to_string(), "yyy".to_string()),
        ]);
        _check_hashmap_element(&expected, actual);
    }

    #[test]
    /// loading test to channel_abbrevations.txt
    fn test_load_abbrevations() {
        let actual = create_output_filter_config("test_files/config/channel_abbreviations.txt");
        let actual2 = create_output_filter_config("test_files/config/channel_abbreviations.txt");
        let expected: HashMap<String, String> = HashMap::from([
            ("Security".to_ascii_lowercase(), "Sec".to_string()),
            ("xxx".to_string(), "yyy".to_string()),
        ]);
        _check_hashmap_element(&expected, actual);
        _check_hashmap_element(&expected, actual2);
    }

    #[test]
    fn _get_default_defails() {
        let expected: HashMap<String, String> = HashMap::from([
            ("Microsoft-Windows-PowerShell_4104".to_string(),"%ScriptBlockText%".to_string()),("Microsoft-Windows-Security-Auditing_4624".to_string(), "User: %TargetUserName% | Comp: %WorkstationName% | IP Addr: %IpAddress% | LID: %TargetLogonId% | Process: %ProcessName%".to_string()),
            ("Microsoft-Windows-Sysmon_1".to_string(), "Cmd: %CommandLine% | Process: %Image% | User: %User% | Parent Cmd: %ParentCommandLine% | LID: %LogonId% | PID: %ProcessId% | PGUID: %ProcessGuid%".to_string()),
            ("Service Control Manager_7031".to_string(), "Svc: %param1% | Crash Count: %param2% | Action: %param5%".to_string()),
        ]);
        let actual = get_default_details("test_files/config/default_details.txt");
        _check_hashmap_element(&expected, actual);
    }

    /// check two HashMap element length and value
    fn _check_hashmap_element(expected: &HashMap<String, String>, actual: HashMap<String, String>) {
        assert_eq!(expected.len(), actual.len());
        for (k, v) in expected.iter() {
            assert!(actual.get(k).unwrap_or(&String::default()) == v);
        }
    }

    #[test]
    fn test_insert_message_race_condition() {
        MESSAGES.clear();

        // Setup test detect_info before starting threads.
        let mut sample_detects = vec![];
        let mut rng = rand::thread_rng();
        let sample_event_time = Utc::now();
        for i in 1..2001 {
            let detect_info = DetectInfo {
                rulepath: "".to_string(),
                ruletitle: "".to_string(),
                level: "".to_string(),
                computername: "".to_string(),
                eventid: i.to_string(),
                detail: "".to_string(),
                record_information: None,
                ext_field: Default::default(),
            };
            sample_detects.push((sample_event_time, detect_info, rng.gen_range(0..10)));
        }

        // Starting threads and randomly insert_message in parallel.
        let mut handles = vec![];
        for (event_time, detect_info, random_num) in sample_detects {
            let handle = thread::spawn(move || {
                thread::sleep(Duration::from_micros(random_num));
                insert_message(detect_info, event_time);
            });
            handles.push(handle);
        }

        // Wait for all threads execution completion.
        for handle in handles {
            handle.join().unwrap();
        }

        // Expect all sample_detects to be included, but the len() size will be different each time I run it
        assert_eq!(get(sample_event_time).len(), 2000)
    }
}
