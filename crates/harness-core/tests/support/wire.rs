use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Read, Write},
    net::TcpListener,
    thread::JoinHandle,
};
pub struct Wire {
    pub status: u16,
    pub kind: &'static str,
    pub body: String,
    pub headers: String,
}
impl Wire {
    pub fn sse(body: String) -> Self {
        Self {
            status: 200,
            kind: "text/event-stream",
            body,
            headers: String::new(),
        }
    }
}
pub fn event(delta: Value, finish: Value) -> String {
    format!(
        "data: {}\r\n\r\n",
        json!({"id":"gen-fixture","model":"fixture/model","provider":"Fixture","choices":[{"index":0,"delta":delta,"finish_reason":finish}]})
    )
}
pub fn serve(responses: Vec<Wire>) -> (String, JoinHandle<Vec<Value>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = std::thread::spawn(move || {
        let catalog = json!({"data":[{"id":"fixture/model","canonical_slug":"fixture/model","context_length":128000,"supported_parameters":["tools","max_tokens","reasoning"],"architecture":{"output_modalities":["text"]},"reasoning":{"supported_efforts":["high"]}}]});
        let endpoints = json!({"data":{"endpoints":[{"tag":"fixture","provider_name":"Fixture","context_length":128000,"max_completion_tokens":4096,"supported_parameters":["tools","max_tokens","reasoning"]}]}});
        let mut catalog = catalog;
        let mut alias = catalog["data"][0].clone();
        alias["id"] = json!("~fixture/latest");
        alias["canonical_slug"] = json!("~fixture/latest");
        alias["alias_target"] = json!({"slug":"fixture/model"});
        catalog["data"].as_array_mut().unwrap().push(alias);
        let mut endpoints = endpoints;
        let mut duplicate = endpoints["data"]["endpoints"][0].clone();
        duplicate["tag"] = json!("fixture/spot");
        endpoints["data"]["endpoints"]
            .as_array_mut()
            .unwrap()
            .push(duplicate);
        let mut regional = endpoints["data"]["endpoints"][0].clone();
        regional["tag"] = json!("fixture/us");
        endpoints["data"]["endpoints"]
            .as_array_mut()
            .unwrap()
            .push(regional);
        let mut other = endpoints["data"]["endpoints"][0].clone();
        other["tag"] = json!("fixture-other");
        other["provider_name"] = json!("Other");
        endpoints["data"]["endpoints"]
            .as_array_mut()
            .unwrap()
            .push(other);
        let mut all = vec![
            Wire {
                status: 200,
                kind: "application/json",
                body: catalog.to_string(),
                headers: String::new(),
            },
            Wire {
                status: 200,
                kind: "application/json",
                body: endpoints.to_string(),
                headers: String::new(),
            },
        ];
        all.extend(responses);
        let mut requests = Vec::new();
        for (index, response) in all.into_iter().enumerate() {
            let (stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(10)))
                .unwrap();
            let mut reader = BufReader::new(stream);
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let mut size = 0;
            let mut auth = None;
            loop {
                line.clear();
                reader.read_line(&mut line).unwrap();
                if line == "\r\n" {
                    break;
                }
                let lower = line.to_lowercase();
                if let Some(v) = lower.strip_prefix("content-length:") {
                    size = v.trim().parse::<usize>().unwrap();
                }
                if let Some(v) = lower.strip_prefix("authorization:") {
                    auth = Some(v.trim().to_owned());
                }
            }
            assert!(size < 2 * 1024 * 1024);
            let mut body = vec![0; size];
            reader.read_exact(&mut body).unwrap();
            if index < 2 {
                assert!(auth.is_none(), "catalog must not receive credentials");
            } else {
                assert_eq!(auth.as_deref(), Some("bearer fixture-not-a-secret"));
                requests.push(serde_json::from_slice(&body).unwrap());
            }
            let mut stream = reader.into_inner();
            write!(stream,"HTTP/1.1 {} Fixture\r\nContent-Type: {}\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n{}\r\n",response.status,response.kind,response.headers).unwrap();
            // Deliberately fragment framing, UTF-8 and tool JSON at the external boundary.
            for chunk in response.body.as_bytes().chunks(7) {
                if write!(stream, "{:x}\r\n", chunk.len())
                    .and_then(|_| stream.write_all(chunk))
                    .and_then(|_| stream.write_all(b"\r\n"))
                    .is_err()
                {
                    break;
                }
            }
            let _ = stream.write_all(b"0\r\n\r\n");
        }
        requests
    });
    (base, server)
}
