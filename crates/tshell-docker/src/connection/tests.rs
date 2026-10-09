use super::*;
use std::{sync::Arc, time::Duration};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[derive(Default)]
struct Fixture {
    requests: parking_lot::Mutex<Vec<(String, serde_json::Value)>>,
    input: parking_lot::Mutex<Vec<u8>>,
    closed: tokio::sync::Notify,
    hang: std::sync::atomic::AtomicBool,
}
fn docker(fixture: Arc<Fixture>, running: bool) -> Docker {
    Docker::connect_with_custom_transport(move |request| {
        let fixture = fixture.clone();
        async move {
            let (client, mut server) = tokio::io::duplex(64 * 1024);
            tokio::spawn(async move {
                let mut bytes = Vec::new();
                let mut byte = [0];
                while !bytes.ends_with(b"\r\n\r\n") {
                    server.read_exact(&mut byte).await.unwrap();
                    bytes.push(byte[0]);
                }
                let headers = String::from_utf8(bytes).unwrap();
                let first = headers.lines().next().unwrap();
                let path = first.split_whitespace().nth(1).unwrap().to_owned();
                assert!(path.starts_with('/'), "HTTP origin form required: {path}");
                assert!(headers.to_lowercase().contains("host: docker\r\n"));
                let length: usize = headers.lines().find_map(|line| {
                    let (key,value) = line.split_once(':')?;
                    key.eq_ignore_ascii_case("content-length").then(|| value.trim().parse().unwrap())
                }).unwrap_or(0);
                let mut body = vec![0;length];
                server.read_exact(&mut body).await.unwrap();
                let body = serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null);
                fixture.requests.lock().push((path.clone(), body));
                if fixture.hang.load(std::sync::atomic::Ordering::Acquire) {
                    let mut byte = [0];
                    assert_eq!(server.read(&mut byte).await.unwrap(),0);
                    fixture.closed.notify_one();
                    return;
                }
                if path.ends_with("/start") {
                    server.write_all(b"HTTP/1.1 101 Switching Protocols\r\nConnection: Upgrade\r\nUpgrade: tcp\r\nContent-Type: application/vnd.docker.raw-stream\r\n\r\n").await.unwrap();
                    server.write_all("ready 世界\r\n".as_bytes()).await.unwrap();
                    let mut buffer = [0;1024];
                    while let Ok(count) = server.read(&mut buffer).await {
                        if count == 0 { break; }
                        fixture.input.lock().extend_from_slice(&buffer[..count]);
                        if server.write_all(&buffer[..count]).await.is_err() { break; }
                    }
                    fixture.closed.notify_one();
                    return;
                }
                let (status,body) = if path == "/version" {
                    ("200 OK", r#"{"ApiVersion":"1.53","MinAPIVersion":"1.24"}"#.to_owned())
                } else if path.ends_with("/json") {
                    ("200 OK", format!(r#"{{"Id":"container-full-id","State":{{"Running":{running}}}}}"#))
                } else if path.ends_with("/exec") {
                    ("201 Created", r#"{"Id":"fixture-exec"}"#.to_owned())
                } else if path.contains("/resize?") {
                    ("200 OK", String::new())
                } else {
                    ("404 Not Found", r#"{"message":"fixture missing endpoint"}"#.to_owned())
                };
                let response = format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len());
                server.write_all(response.as_bytes()).await.unwrap();
            });
            request_over_stream(client,request).await
        }
    },Some("http://fixture"),5,API_DEFAULT_VERSION).unwrap()
}
#[test]
fn sdk_negotiates_and_hijacks_exec_with_input_resize_and_cancellation() {
    tshell_host::ssh::runtime().block_on(async {
        tokio::time::timeout(Duration::from_secs(10), async {
            let fixture = Arc::new(Fixture::default());
            let docker = docker(fixture.clone(), true)
                .negotiate_version()
                .await
                .unwrap();
            let io = crate::terminal::connect(
                docker,
                "container-full-id".into(),
                vec!["/bin/sh".into()],
                24,
                80,
            )
            .await
            .unwrap();
            assert_eq!(
                io.output.recv().await.unwrap().unwrap(),
                "ready 世界\r\n".as_bytes()
            );
            io.input.resize(40, 120);
            io.input.send(b"echo hello\r".to_vec()).unwrap();
            assert_eq!(io.output.recv().await.unwrap().unwrap(), b"echo hello\r");
            assert_eq!(*fixture.input.lock(), b"echo hello\r");
            let requests = fixture.requests.lock().clone();
            let exec = requests
                .iter()
                .find(|(path, _)| path.ends_with("/exec"))
                .unwrap();
            assert_eq!(exec.1["Cmd"], serde_json::json!(["/bin/sh"]));
            assert_eq!(exec.1["Tty"], true);
            assert!(requests.iter().any(|(path, _)| path.contains("/resize?")
                && path.contains("h=40")
                && path.contains("w=120")));
            assert!(
                requests
                    .iter()
                    .all(|(path, _)| !path.contains("/stop") && !path.contains("/kill"))
            );
            drop(io);
            fixture.closed.notified().await;
        })
        .await
        .unwrap();
    });
}
#[test]
fn stopped_container_rejects_exec_before_creating_it() {
    tshell_host::ssh::runtime().block_on(async {
        let fixture = Arc::new(Fixture::default());
        let result = crate::terminal::connect(
            docker(fixture.clone(), false),
            "container-full-id".into(),
            vec!["/bin/sh".into()],
            24,
            80,
        )
        .await;
        assert!(result.is_err());
        assert_eq!(fixture.requests.lock().len(), 1);
    });
}

#[test]
fn cancelled_request_closes_transport_before_response_headers() {
    tshell_host::ssh::runtime().block_on(async {
        let fixture = Arc::new(Fixture::default());
        fixture
            .hang
            .store(true, std::sync::atomic::Ordering::Release);
        let docker = docker(fixture.clone(), true);
        assert!(
            tokio::time::timeout(Duration::from_millis(100), docker.version())
                .await
                .is_err()
        );
        tokio::time::timeout(Duration::from_secs(2), fixture.closed.notified())
            .await
            .unwrap();
    });
}
