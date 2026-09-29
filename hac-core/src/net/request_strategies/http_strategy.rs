use crate::collection::types::{Request, RequestMethod};
use crate::net::request_client::RequestClient;
use crate::net::request_manager::Response;
use crate::net::request_strategies::RequestStrategy;
use crate::net::response_decoders::{decoder_from_headers, ResponseDecoder};

pub struct HttpResponse;

impl RequestStrategy for HttpResponse {
    async fn handle(&self, request: Request) -> Response {
        let client = RequestClient::default();

        match request.method {
            RequestMethod::Get => self.handle_get_request(client, request).await,
            RequestMethod::Post => self.handle_post_request(client, request).await,
            RequestMethod::Put => self.handle_put_request(client, request).await,
            RequestMethod::Patch => self.handle_patch_request(client, request).await,
            RequestMethod::Delete => self.handle_delete_request(client, request).await,
        }
    }
}

impl HttpResponse {
    async fn handle_get_request(&self, client: RequestClient, request: Request) -> Response {
        let now = std::time::Instant::now();
        match client.get(&request).send().await {
            Ok(response) => {
                let decoder = decoder_from_headers(response.headers());
                decoder.decode(response, now).await
            }
            Err(e) => Response {
                is_error: true,
                cause: Some(e.to_string()),
                body: None,
                pretty_body: None,
                body_size: None,
                size: None,
                headers_size: None,
                status: None,
                headers: None,
                duration: now.elapsed(),
            },
        }
    }

    async fn handle_post_request(&self, client: RequestClient, request: Request) -> Response {
        let now = std::time::Instant::now();
        match client
            .post(&request)
            .body(request.body.unwrap_or_default())
            .send()
            .await
        {
            Ok(response) => {
                let decoder = decoder_from_headers(response.headers());
                decoder.decode(response, now).await
            }
            Err(e) => Response {
                is_error: true,
                cause: Some(e.to_string()),
                body: None,
                pretty_body: None,
                body_size: None,
                size: None,
                headers_size: None,
                status: None,
                headers: None,
                duration: now.elapsed(),
            },
        }
    }

    async fn handle_put_request(&self, client: RequestClient, request: Request) -> Response {
        let now = std::time::Instant::now();
        match client
            .put(&request)
            .body(request.body.unwrap_or_default())
            .send()
            .await
        {
            Ok(response) => {
                let decoder = decoder_from_headers(response.headers());
                decoder.decode(response, now).await
            }
            Err(e) => Response {
                is_error: true,
                cause: Some(e.to_string()),
                body: None,
                pretty_body: None,
                body_size: None,
                size: None,
                headers_size: None,
                status: None,
                headers: None,
                duration: now.elapsed(),
            },
        }
    }

    async fn handle_patch_request(&self, client: RequestClient, request: Request) -> Response {
        let now = std::time::Instant::now();
        match client
            .patch(&request)
            .body(request.body.unwrap_or_default())
            .send()
            .await
        {
            Ok(response) => {
                let decoder = decoder_from_headers(response.headers());
                decoder.decode(response, now).await
            }
            Err(e) => Response {
                is_error: true,
                cause: Some(e.to_string()),
                body: None,
                pretty_body: None,
                body_size: None,
                size: None,
                headers_size: None,
                status: None,
                headers: None,
                duration: now.elapsed(),
            },
        }
    }

    async fn handle_delete_request(&self, client: RequestClient, request: Request) -> Response {
        let now = std::time::Instant::now();
        match client
            .delete(&request)
            .body(request.body.unwrap_or_default())
            .send()
            .await
        {
            Ok(response) => {
                let decoder = decoder_from_headers(response.headers());
                decoder.decode(response, now).await
            }
            Err(e) => Response {
                is_error: true,
                cause: Some(e.to_string()),
                body: None,
                pretty_body: None,
                body_size: None,
                size: None,
                headers_size: None,
                status: None,
                headers: None,
                duration: now.elapsed(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;

    use tokio::sync::mpsc;

    use crate::collection::types::{BodyType, Request, RequestMethod};
    use crate::net::request_manager::handle_request;

    use super::*;

    /// a bare bones http server that reads a single request and returns the
    /// raw bytes it received on the body, headers are ignored
    fn spawn_raw_server() -> (String, std::thread::JoinHandle<Vec<u8>>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("failed to bind test server");
        let addr = listener.local_addr().expect("failed to get test server addr");

        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener
                .accept()
                .expect("failed to accept connection on test server");

            let mut buffer = [0u8; 4096];
            let mut raw_request = Vec::new();

            // read until we have the whole body as advertised by
            // content-length, then reply with an empty 200 so the client
            // completes its request
            loop {
                if let Some(end_of_headers) = find_end_of_headers(&raw_request) {
                    let headers =
                        String::from_utf8_lossy(&raw_request[..end_of_headers]).to_string();
                    if let Some(length) = headers.lines().find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().ok())
                            .flatten()
                    }) {
                        let body_start = end_of_headers + 4;
                        if raw_request.len() >= body_start + length {
                            let body = raw_request[body_start..body_start + length].to_vec();
                            let _ = stream
                                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n");
                            return body;
                        }
                    }
                }

                let read = stream
                    .read(&mut buffer)
                    .expect("failed to read from test server");
                if read.eq(&0) {
                    break;
                }
                raw_request.extend_from_slice(&buffer[..read]);
            }

            Vec::new()
        });

        (format!("http://{addr}/"), handle)
    }

    fn find_end_of_headers(raw: &[u8]) -> Option<usize> {
        raw.windows(4).position(|window| window.eq(b"\r\n\r\n"))
    }

    fn build_request(uri: String, body: &str) -> Request {
        Request {
            id: "test".to_string(),
            method: RequestMethod::Post,
            name: "test".to_string(),
            uri,
            headers: None,
            auth_method: None,
            parent: None,
            body: Some(body.to_string()),
            body_type: Some(BodyType::Json),
        }
    }

    /// sending a json body must put the raw json text on the wire, the
    /// strategy used to double-encode the body by serializing the string
    /// itself with `.json()`, wrapping the body in quotes and escapes
    #[tokio::test]
    async fn test_post_sends_raw_json_body() {
        let (uri, server) = spawn_raw_server();
        let request = build_request(uri, r#"{"key": "value"}"#);

        let (tx, mut rx) = mpsc::unbounded_channel();
        handle_request(
            &std::sync::Arc::new(std::sync::RwLock::new(request)),
            tx,
        );
        // the response is received once the request is done, so waiting on
        // it guarantees the server saw the whole request
        let _response = rx.recv().await;
        let received_body = server.join().expect("test server panicked");

        assert_eq!(received_body, br#"{"key": "value"}"#);
    }
}
