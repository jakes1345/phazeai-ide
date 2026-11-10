//! A blocking HTTP/1.1 server: one thread per connection, one request each.

use std::io::{self, BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::thread;

use crate::config::Config;
use crate::log;
use crate::routes::{self, Request};
use crate::store::Store;

const MAX_HEADER_BYTES: usize = 8 * 1024;

pub fn serve(config: &Config, store: Store) -> io::Result<()> {
    let listener = TcpListener::bind(config.bind_address())?;
    log::info(&format!("listening on {}", listener.local_addr()?));
    let store = Arc::new(store);
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let store = Arc::clone(&store);
                thread::spawn(move || {
                    if let Err(err) = handle_connection(stream, &store) {
                        log::debug(&format!("connection error: {err}"));
                    }
                });
            }
            Err(err) => log::warn(&format!("accept failed: {err}")),
        }
    }
    Ok(())
}

fn handle_connection(stream: TcpStream, store: &Store) -> io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut request_line = String::new();
    reader.read_line(&mut request_line)?;

    // Drain the headers; tally needs none of them.
    let mut header_bytes = 0;
    loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line)?;
        header_bytes += n;
        if n == 0 || line == "\r\n" || line == "\n" {
            break;
        }
        if header_bytes > MAX_HEADER_BYTES {
            return write_response(stream, 400, "Bad Request", "headers too large\n");
        }
    }

    let mut parts = request_line.split_whitespace();
    let (Some(method), Some(path)) = (parts.next(), parts.next()) else {
        return write_response(stream, 400, "Bad Request", "malformed request line\n");
    };
    log::debug(&format!("{method} {path}"));

    let response = routes::handle(store, &Request { method, path });
    let reason = response.reason();
    write_response(stream, response.status, reason, &response.body)
}

fn write_response(mut stream: TcpStream, status: u16, reason: &str, body: &str) -> io::Result<()> {
    write!(
        stream,
        "HTTP/1.1 {status} {reason}\r\ncontent-type: text/plain\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    )?;
    stream.flush()
}
