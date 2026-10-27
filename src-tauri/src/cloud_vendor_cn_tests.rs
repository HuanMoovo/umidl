use super::*;
use serde_json::json;

#[test]
fn tencent_sentences_use_real_millisecond_timestamps() {
    let cues = parse_tencent(&json!({"code":0,"flash_result":[{"channel_id":0,"text":"你好。","sentence_list":[{"text":"你好。","start_time":230,"end_time":1480}]}]})).unwrap();
    assert_eq!((cues[0].start_ms, cues[0].end_ms, cues[0].text.as_str()), (230, 1480, "你好。"));
}

#[test]
fn baidu_sentences_use_real_millisecond_timestamps() {
    let cues = parse_baidu(&json!({"error_code":0,"result":["你好"],"detailed_result":[{"begin_time":120,"end_time":850,"res":["你好"]}]})).unwrap();
    assert_eq!((cues[0].start_ms, cues[0].end_ms, cues[0].text.as_str()), (120, 850, "你好"));
}

#[test]
fn xunfei_uses_sentence_offsets_and_joins_words_and_punctuation() {
    let sentence = json!({"st":{"bg":"120","ed":"980","rt":[{"ws":[{"cw":[{"w":"你好"}]},{"cw":[{"w":"。"}]}]}]}}).to_string();
    let result = json!({"lattice":[{"json_1best":sentence}]}).to_string();
    let cues = parse_xunfei_result(&result).unwrap();
    assert_eq!((cues[0].start_ms, cues[0].end_ms, cues[0].text.as_str()), (120, 980, "你好。"));
}

#[test]
fn no_provider_fabricates_missing_timestamps() {
    assert!(parse_tencent(&json!({"code":0,"flash_result":[{"text":"有文字","sentence_list":[]}]})).is_err());
    assert!(parse_baidu(&json!({"error_code":0,"result":["有文字"],"detailed_result":[]})).is_err());
    let sentence = json!({"st":{"bg":"0","rt":[{"ws":[{"cw":[{"w":"文字"}]}]}]}}).to_string();
    assert!(parse_xunfei_result(&json!({"lattice":[{"json_1best":sentence}]}).to_string()).is_err());
}

#[test]
fn empty_speech_chunks_are_skipped_but_malformed_responses_fail() {
    assert!(parse_tencent(&json!({"code":0,"flash_result":[]})).unwrap().is_empty());
    assert!(parse_tencent(&json!({"code":0})).is_err());
    assert!(parse_baidu(&json!({"error_code":0,"result":[]})).unwrap().is_empty());
    assert!(parse_baidu(&json!({"error_code":0})).is_err());
    assert!(parse_xunfei_result(&json!({"lattice":[]}).to_string()).unwrap().is_empty());
}

#[test]
fn invalid_sentence_among_valid_sentences_cannot_be_silently_dropped() {
    assert!(parse_tencent(&json!({"code":0,"flash_result":[{"text":"甲乙","sentence_list":[
        {"text":"甲","start_time":0,"end_time":10},{"start_time":10,"end_time":20}]}]})).is_err());
    assert!(parse_baidu(&json!({"error_code":0,"result":["甲乙"],"detailed_result":[
        {"res":["甲"],"begin_time":0,"end_time":10},{"begin_time":10,"end_time":20}]})).is_err());
    let good = json!({"st":{"bg":"0","ed":"10","rt":[{"ws":[{"cw":[{"w":"甲"}]}]}]}}).to_string();
    let bad = json!({"st":{"bg":"10","ed":"20","rt":[{}]}}).to_string();
    assert!(parse_xunfei_result(&json!({"lattice":[{"json_1best":good},{"json_1best":bad}]}).to_string()).is_err());
}

#[test]
fn hmac_sha1_matches_rfc_2202_test_vector() {
    assert_eq!(sign_sha1(&[0x0b;20], b"Hi There"), "thcxhlUFcmTii8C2+zeMjvFGvgA=");
}

#[test]
fn tencent_signing_uses_sorted_query_and_official_canonical_host() {
    let items = vec![("voice_format", "wav"), ("secretid", "example"), ("engine_type", "16k_zh"), ("timestamp", "1700000000")];
    let (canonical, query) = tencent_signing_parts("123", &items);
    assert_eq!(query, "engine_type=16k_zh&secretid=example&timestamp=1700000000&voice_format=wav");
    assert_eq!(canonical, format!("POSTasr.cloud.tencent.com/asr/flash/v1/123?{query}"));
}

#[test]
fn xunfei_signing_escapes_values_and_orders_parameters() {
    let items = vec![("fileName", "你好.wav"), ("accessKeyId", "abc"), ("dateTime", "2026-10-27T12:00:00+0800")];
    let canonical = xunfei_signing_query(&items);
    assert_eq!(canonical, "accessKeyId=abc&dateTime=2026-10-27T12%3A00%3A00%2B0800&fileName=%E4%BD%A0%E5%A5%BD.wav");
}

#[derive(Debug)]
struct CapturedRequest { first_line: String, headers: String, body: Vec<u8> }

fn mock_server(responses: Vec<Value>) -> (String, std::thread::JoinHandle<Vec<CapturedRequest>>) {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let worker = std::thread::spawn(move || {
        let mut requests = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(10);
        for value in responses {
            let mut stream = loop {
                match listener.accept() {
                    Ok((socket, _)) => break socket,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
                    _ => return requests,
                }
            };
            stream.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
            let mut data = Vec::new();
            let mut chunk = [0u8; 2048];
            let header_end = loop {
                let count = stream.read(&mut chunk).unwrap();
                assert!(count > 0, "request ended before headers");
                data.extend_from_slice(&chunk[..count]);
                if let Some(pos) = data.windows(4).position(|part| part == b"\r\n\r\n") { break pos + 4; }
                assert!(data.len() < 65536, "headers exceeded test limit");
            };
            let headers = String::from_utf8(data[..header_end].to_vec()).unwrap();
            let body_length: usize = headers.lines().find_map(|line| {
                let (name, value) = line.split_once(':')?;
                if name.eq_ignore_ascii_case("content-length") { value.trim().parse().ok() } else { None }
            }).unwrap_or(0);
            while data.len() < header_end + body_length {
                let count = stream.read(&mut chunk).unwrap();
                assert!(count > 0, "request ended before body");
                data.extend_from_slice(&chunk[..count]);
            }
            let first_line = headers.lines().next().unwrap().to_owned();
            requests.push(CapturedRequest { first_line, headers, body: data[header_end..header_end+body_length].to_vec() });
            let body = value.to_string();
            write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).unwrap();
        }
        requests
    });
    (url, worker)
}

#[test]
fn tencent_upload_uses_official_path_signing_and_sentence_times() {
    let (base, server) = mock_server(vec![json!({"code":0,"flash_result":[{"text":"腾讯。","sentence_list":[{"text":"腾讯。","start_time":20,"end_time":900}]}]})]);
    let cfg = CloudAsrConfig { provider:"tencent".into(), base_url:base, app_id:"12345".into(), secret_id:"test-id".into(), secret_key:"test-secret".into(), model:"16k_zh".into(), ..Default::default() };
    let result = execute("tencent", &cfg, "zh", b"fake-wav".to_vec(), Arc::new(AtomicBool::new(false))).unwrap();
    let captured = server.join().unwrap();
    assert_eq!(captured.len(), 1);
    assert!(captured[0].first_line.starts_with("POST /asr/flash/v1/12345?"));
    assert!(captured[0].first_line.contains("engine_type=16k_zh"));
    assert!(captured[0].first_line.contains("voice_format=wav"));
    assert!(captured[0].headers.to_ascii_lowercase().contains("authorization:"));
    assert!(!captured[0].first_line.contains("test-secret"));
    assert_eq!(captured[0].body, b"fake-wav");
    assert_eq!((result[0].start_ms, result[0].end_ms), (20, 900));
}

#[test]
fn baidu_gets_session_token_then_uploads_audio_with_subtitle_flag() {
    let (base, server) = mock_server(vec![json!({"access_token":"fake-access-token"}),
        json!({"error_code":0,"result":["百度。"],"detailed_result":[{"res":["百度。"],"begin_time":60,"end_time":760}]})]);
    let cfg = CloudAsrConfig { provider:"baidu".into(), base_url:base, api_key:"fake+key".into(), secret_key:"fake/secret+".into(), model:"80006".into(), ..Default::default() };
    let result = execute("baidu", &cfg, "zh", b"fake-wav".to_vec(), Arc::new(AtomicBool::new(false))).unwrap();
    let captured = server.join().unwrap();
    assert_eq!(captured.len(), 2);
    assert!(captured[0].first_line.starts_with("POST /oauth/2.0/token?"));
    assert!(captured[0].first_line.contains("client_id=fake%2Bkey"));
    assert!(captured[0].first_line.contains("client_secret=fake%2Fsecret%2B"));
    assert!(captured[0].body.is_empty());
    assert!(captured[1].first_line.contains("/rest/2.0/speech/publiccloudspeech/v1/asr/topspeed"));
    assert!(!captured[1].first_line.contains("client_secret"));
    let upload:Value = serde_json::from_slice(&captured[1].body).unwrap();
    assert_eq!(upload["access_token"], "fake-access-token");
    assert_eq!(upload["dev_pid"], 80006);
    assert_eq!(upload["enable_subtitle"], 1);
    assert_eq!(STANDARD.decode(upload["speech"].as_str().unwrap()).unwrap(), b"fake-wav");
    assert_eq!((result[0].start_ms, result[0].end_ms), (60, 760));
}

#[test]
fn xunfei_upload_and_result_poll_use_same_random_and_timed_result() {
    let sentence = json!({"st":{"bg":"150","ed":"1050","rt":[{"ws":[{"cw":[{"w":"讯飞"}]},{"cw":[{"w":"。"}]}]}]}}).to_string();
    let output = json!({"lattice":[{"json_1best":sentence}]}).to_string();
    let (base, server) = mock_server(vec![json!({"code":"000000","content":{"orderId":"order-123"}}),
        json!({"code":"000000","content":{"orderInfo":{"status":4},"orderResult":output}})]);
    let cfg = CloudAsrConfig { provider:"xunfei".into(), base_url:base, app_id:"fake-app".into(), api_key:"fake-key".into(), secret_key:"fake-secret".into(), model:"autodialect".into(), ..Default::default() };
    let result = execute("xunfei", &cfg, "auto", b"fake-wav".to_vec(), Arc::new(AtomicBool::new(false))).unwrap();
    let captured = server.join().unwrap();
    assert_eq!(captured.len(), 2);
    assert!(captured[0].first_line.starts_with("POST /v2/upload?"));
    assert!(captured[1].first_line.starts_with("POST /v2/getResult?"));
    assert!(captured[0].first_line.contains("language=autodialect"));
    assert!(captured[1].first_line.contains("orderId=order-123"));
    assert!(captured.iter().all(|r| r.headers.to_ascii_lowercase().contains("signature:")));
    assert!(captured.iter().all(|r| !r.first_line.contains("fake-secret")));
    let random = |line: &str| -> String { line.split("signatureRandom=").nth(1).unwrap().split('&').next().unwrap().split(' ').next().unwrap().to_owned() };
    assert_eq!(random(&captured[0].first_line), random(&captured[1].first_line));
    assert_eq!(captured[0].body, b"fake-wav");
    assert_eq!(captured[1].body, b"{}");
    assert_eq!((result[0].start_ms, result[0].end_ms, result[0].text.as_str()), (150, 1050, "讯飞。"));
}

#[test]
fn provider_errors_never_include_echoed_secret() {
    let (base, server) = mock_server(vec![json!({"code":4001,"message":"synthetic-secret-echo"})]);
    let cfg = CloudAsrConfig { provider:"tencent".into(), base_url:base, app_id:"12345".into(), secret_id:"synthetic-id".into(), secret_key:"synthetic-secret-echo".into(), model:"16k_zh".into(), ..Default::default() };
    let error = execute("tencent", &cfg, "zh", b"fake-wav".to_vec(), Arc::new(AtomicBool::new(false))).unwrap_err().to_string();
    assert!(!error.contains("synthetic-secret-echo"));
    assert!(error.contains("4001"));
    server.join().unwrap();
}

#[test]
fn cancelled_upload_reader_fails_without_retryable_interrupt() {
    let flag = Arc::new(AtomicBool::new(true));
    let mut reader = CancelReader { bytes: Cursor::new(vec![1, 2, 3]), canceled: flag };
    assert_eq!(reader.read(&mut [0u8; 3]).unwrap_err().kind(), std::io::ErrorKind::Other);
}

#[test]
fn native_endpoint_rejects_unknown_remote_hosts_and_credentials_in_urls() {
    let cfg = CloudAsrConfig { provider:"baidu".into(), base_url:"https://evil.example/".into(), ..Default::default() };
    assert!(endpoint_base(&cfg).is_err());
    let cfg = CloudAsrConfig { base_url:"https://secret@aip.baidubce.com".into(), ..cfg };
    assert!(endpoint_base(&cfg).is_err());
    let cfg = CloudAsrConfig { base_url:"https://@aip.baidubce.com".into(), ..cfg };
    assert!(endpoint_base(&cfg).is_err());
}

#[test]
fn cancellation_returns_promptly_and_releases_chunk_while_response_is_blocked() {
    use crate::ctx::{AppDirs, Ctx, ToolPaths};
    use crate::models::AppSettings;
    use std::net::TcpListener;
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base_url = format!("http://{}", listener.local_addr().unwrap());
    let (accepted_tx, accepted_rx) = mpsc::sync_channel(1);
    let server = std::thread::spawn(move || {
        let (_socket, _) = listener.accept().unwrap();
        accepted_tx.send(()).unwrap();
        // Keep the request open and do not consume its body.
        std::thread::sleep(Duration::from_secs(2));
    });
    let dir = std::env::temp_dir();
    let path = dir.join(format!("umidl-cn-asr-cancel-{}.wav", uuid::Uuid::new_v4()));
    std::fs::write(&path, vec![0u8; 3 * 1024 * 1024]).unwrap();
    let dirs = AppDirs { data:dir.clone(), bin:dir.clone(), models:dir.clone(), cache:dir.clone(),
        downloads:dir.clone(), db_file:dir.join("unused.db"), settings_file:dir.join("unused.json") };
    let ctx = Ctx { dirs, tools:ToolPaths::default(), settings:AppSettings::default(), emit:None };
    let cfg = CloudAsrConfig { provider:"tencent".into(), base_url, app_id:"12345".into(), secret_id:"test-id".into(), secret_key:"test-secret".into(), model:"16k_zh".into(), ..Default::default() };
    let started = Instant::now();
    let result = transcribe(&ctx, &path, &cfg, "zh", &|| accepted_rx.try_recv().is_ok());
    let elapsed = started.elapsed();
    assert!(result.is_err());
    assert!(elapsed < Duration::from_millis(1500), "cancel waited for blocked network response: {elapsed:?}");
    std::fs::remove_file(path).expect("chunk must not remain open after cancellation");
    server.join().unwrap();
}
