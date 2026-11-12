//! A loopback HTTP server for tests: `reply` turns a request path into a
//! whole raw response, and every request line is recorded.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

pub struct Server {
    pub base: String,
    pub seen: Arc<Mutex<Vec<String>>>,
}

pub fn serve(reply: impl Fn(&str) -> String + Send + Sync + 'static) -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    let reply = Arc::new(reply);
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut head = String::new();
            let mut length = 0;
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                    break;
                }
                if let Some(v) = line.to_lowercase().strip_prefix("content-length:") {
                    length = v.trim().parse().unwrap_or(0);
                }
                head.push_str(&line);
            }
            let mut body = vec![0; length];
            let _ = reader.read_exact(&mut body);
            log.lock().unwrap().push(head.clone());
            let path = head.split_whitespace().nth(1).unwrap_or("").to_string();
            let mut stream = stream;
            let _ = stream.write_all(reply(&path).as_bytes());
        }
    });
    Server { base, seen }
}

pub fn ok(body: &str) -> String {
    format!(
        "HTTP/1.1 200 OK\r\ncontent-type: text/plain\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    )
}

pub fn redirect(to: &str) -> String {
    format!(
        "HTTP/1.1 307 Temporary Redirect\r\nlocation: {to}\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
    )
}
