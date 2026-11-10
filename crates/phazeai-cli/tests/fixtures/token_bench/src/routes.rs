//! Maps a parsed request to a response.

use crate::store::{Store, StoreError};

pub struct Request<'a> {
    pub method: &'a str,
    pub path: &'a str,
}

pub struct Response {
    pub status: u16,
    pub body: String,
}

impl Response {
    fn ok(body: impl Into<String>) -> Response {
        Response {
            status: 200,
            body: body.into(),
        }
    }

    fn status(status: u16, body: &str) -> Response {
        Response {
            status,
            body: format!("{body}\n"),
        }
    }

    pub fn reason(&self) -> &'static str {
        match self.status {
            200 => "OK",
            204 => "No Content",
            400 => "Bad Request",
            404 => "Not Found",
            405 => "Method Not Allowed",
            507 => "Insufficient Storage",
            _ => "Internal Server Error",
        }
    }
}

fn from_store_error(err: StoreError) -> Response {
    match err {
        StoreError::BadName => Response::status(400, "bad counter name"),
        StoreError::NotFound => Response::status(404, "no such counter"),
        StoreError::Full => Response::status(507, "counter limit reached"),
    }
}

pub fn handle(store: &Store, req: &Request<'_>) -> Response {
    if req.path == "/counters" {
        return match req.method {
            "GET" => {
                let mut body = String::new();
                for (name, value) in store.list() {
                    body.push_str(&format!("{name} {value}\n"));
                }
                Response::ok(body)
            }
            _ => Response::status(405, "method not allowed"),
        };
    }

    let Some(name) = req.path.strip_prefix("/counters/") else {
        return Response::status(404, "not found");
    };

    match req.method {
        "GET" => match store.get(name) {
            Ok(v) => Response::ok(format!("{v}\n")),
            Err(e) => from_store_error(e),
        },
        "POST" => match store.bump(name) {
            Ok(v) => Response::ok(format!("{v}\n")),
            Err(e) => from_store_error(e),
        },
        "DELETE" => match store.remove(name) {
            Ok(()) => Response {
                status: 204,
                body: String::new(),
            },
            Err(e) => from_store_error(e),
        },
        _ => Response::status(405, "method not allowed"),
    }
}
