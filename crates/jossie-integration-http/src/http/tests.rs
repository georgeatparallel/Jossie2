#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_public_ip_classification() {
        assert!(is_public_ip("8.8.8.8".parse().unwrap()));
        assert!(!is_public_ip("10.0.0.5".parse().unwrap()));
        assert!(!is_public_ip("100.64.0.1".parse().unwrap()));
        assert!(!is_public_ip("198.18.0.1".parse().unwrap()));
        assert!(!is_public_ip("127.0.0.1".parse().unwrap()));
        assert!(!is_public_ip("::ffff:127.0.0.1".parse().unwrap()));
        assert!(!is_public_ip("::ffff:10.0.0.5".parse().unwrap()));
        assert!(is_public_ip("::ffff:8.8.8.8".parse().unwrap()));
        assert!(is_public_ip("2606:4700:4700::1111".parse().unwrap()));
    }

    #[tokio::test]
    async fn validates_ipv6_literals_without_dns_resolution() {
        for target in ["http://[fc00::1]", "http://[::ffff:127.0.0.1]"] {
            let error = validate_url_target(&Url::parse(target).unwrap())
                .await
                .unwrap_err();
            assert!(error.to_string().contains("local or private IP"));
        }
        validate_url_target(&Url::parse("https://[2606:4700:4700::1111]").unwrap())
            .await
            .unwrap();
    }

    #[test]
    fn log_preview_preserves_utf8_boundaries() {
        let body = format!("{}🌍more", "a".repeat(1999));
        assert_eq!(body_preview(&body, 2000), "a".repeat(1999));
        assert_eq!(body_preview("東京", 1), "");
        assert_eq!(body_preview("café", 5), "café");
        assert_eq!(body_preview("short", 2000), "short");
        assert_eq!(body_preview("", 0), "");
    }

    #[test]
    fn test_redact_headers() {
        let integration = HttpIntegration::new(vec![]);
        let mut headers = HeaderMap::new();
        headers.insert("Authorization", HeaderValue::from_static("Bearer secret"));
        headers.insert("X-Api-Key", HeaderValue::from_static("12345"));
        headers.insert("Cookie", HeaderValue::from_static("session=abc"));
        headers.insert("Content-Type", HeaderValue::from_static("application/json"));

        let redacted = integration.redact_headers(&headers);
        assert!(redacted.contains("authorization"));
        assert!(redacted.contains("[REDACTED]"));
        assert!(!redacted.contains("Bearer secret"));

        assert!(redacted.contains("x-api-key"));
        assert!(!redacted.contains("12345"));

        assert!(redacted.contains("cookie"));
        assert!(!redacted.contains("session=abc"));
        assert!(redacted.contains("content-type"));
        assert!(redacted.contains("application/json"));
    }

    // --- Multipart Integration Tests ---

    use axum::Router;
    use axum::extract::Multipart;
    use axum::routing::post;
    use base64::Engine;
    use tokio::net::TcpListener;

    async fn serve(app: Router) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        format!("http://{addr}")
    }

    fn redirect_app(status: u16, location: &str) -> Router {
        let status = axum::http::StatusCode::from_u16(status).unwrap();
        let location = location.to_string();
        Router::new().route(
            "/redirect",
            axum::routing::any(move || {
                let location = location.clone();
                async move { (status, [(axum::http::header::LOCATION, location)]) }
            }),
        )
    }

    fn echo_app() -> Router {
        Router::new().route(
            "/echo",
            axum::routing::any(
                |method: axum::http::Method, headers: HeaderMap, body: String| async move {
                    axum::Json(serde_json::json!({
                        "method": method.as_str(),
                        "body": body,
                        "headers": headers.iter().map(|(name, value)| {
                            (name.to_string(), value.to_str().unwrap().to_string())
                        }).collect::<HashMap<_, _>>()
                    }))
                },
            ),
        )
    }

    async fn request(
        method: &str,
        url: &str,
        follow_redirects: bool,
        body: BodyContent,
    ) -> anyhow::Result<Value> {
        let output = HttpIntegration::new(vec![])
            .http_request(
                method,
                url,
                Some(HashMap::from([
                    ("Authorization".into(), "Bearer secret".into()),
                    ("Cookie".into(), "session=secret".into()),
                    ("Proxy-Authorization".into(), "Basic secret".into()),
                ])),
                None,
                body,
                Some(2000),
                follow_redirects,
            )
            .await?;
        Ok(serde_json::from_str(&output)?)
    }

    #[tokio::test]
    async fn redirects_preserve_or_drop_method_body_and_headers() {
        for (status, method, expected_method, drops_body) in [
            (301, "POST", "GET", true),
            (302, "POST", "GET", true),
            (301, "PUT", "PUT", false),
            (302, "PATCH", "PATCH", false),
            (303, "PUT", "GET", true),
            (307, "POST", "POST", false),
            (308, "PUT", "PUT", false),
        ] {
            let base = serve(redirect_app(status, "/echo").merge(echo_app())).await;
            let output = request(
                method,
                &format!("{base}/redirect"),
                true,
                BodyContent::Json(b"{}".to_vec()),
            )
            .await
            .unwrap();
            let echoed = &output["body_json"];
            assert_eq!(echoed["method"], expected_method, "status {status}");
            assert_eq!(echoed["body"], if drops_body { "" } else { "{}" });
            assert_eq!(echoed["headers"]["content-type"].is_null(), drops_body);
            assert_eq!(echoed["headers"]["authorization"], "Bearer secret");
        }
    }

    #[test]
    fn redirects_preserve_head_and_ignore_non_redirect_statuses() {
        for status in [301, 302, 303, 307, 308] {
            assert_eq!(
                redirect_method(
                    reqwest::StatusCode::from_u16(status).unwrap(),
                    &reqwest::Method::HEAD
                ),
                Some(reqwest::Method::HEAD)
            );
        }
        for status in [200, 300, 304, 305, 400] {
            assert_eq!(
                redirect_method(
                    reqwest::StatusCode::from_u16(status).unwrap(),
                    &reqwest::Method::GET
                ),
                None
            );
        }
    }

    #[tokio::test]
    async fn cross_origin_redirects_strip_credentials() {
        let destination = serve(echo_app()).await;
        let source = serve(redirect_app(302, &format!("{destination}/echo"))).await;
        let output = request(
            "GET",
            &format!("{source}/redirect"),
            true,
            BodyContent::None,
        )
        .await
        .unwrap();
        for name in ["authorization", "cookie", "proxy-authorization"] {
            assert!(output["body_json"]["headers"][name].is_null(), "{name}");
        }
    }

    #[tokio::test]
    async fn redirects_validate_every_destination() {
        for location in [
            "http://10.0.0.1/private",
            "http://[::ffff:10.0.0.1]/",
            "file:///etc/passwd",
        ] {
            let base = serve(redirect_app(302, location)).await;
            let error = request("GET", &format!("{base}/redirect"), true, BodyContent::None)
                .await
                .unwrap_err();
            assert!(error.to_string().starts_with("Blocked:"), "{error}");
        }
    }

    #[tokio::test]
    async fn redirect_loops_return_an_explicit_error() {
        let base = serve(redirect_app(302, "/redirect")).await;
        let error = request("GET", &format!("{base}/redirect"), true, BodyContent::None)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("redirect limit exceeded"));
    }

    #[tokio::test]
    async fn disabled_redirects_return_the_original_response() {
        let base = serve(redirect_app(302, "http://10.0.0.1/private")).await;
        let output = request("GET", &format!("{base}/redirect"), false, BodyContent::None)
            .await
            .unwrap();
        assert_eq!(output["status"], 302);
    }

    #[tokio::test]
    async fn multipart_redirects_fail_instead_of_sending_an_empty_body() {
        let base = serve(redirect_app(307, "/echo").merge(echo_app())).await;
        let error = request(
            "POST",
            &format!("{base}/redirect"),
            true,
            BodyContent::Multipart(reqwest::multipart::Form::new().text("field", "value")),
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("Cannot replay a multipart body"));
    }

    #[tokio::test]
    async fn invalid_headers_are_rejected_without_exposing_values() {
        let error = HttpIntegration::new(vec![])
            .http_request(
                "GET",
                "http://127.0.0.1:1/",
                Some(HashMap::from([(
                    "Authorization".into(),
                    "secret\nvalue".into(),
                )])),
                None,
                BodyContent::None,
                Some(2000),
                false,
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("Invalid value for HTTP header"));
        assert!(!error.to_string().contains("secret"));
    }

    #[tokio::test]
    async fn test_multipart_upload() {
        // Start a local axum server
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let app = Router::new().route(
            "/upload",
            post(|mut multipart: Multipart| async move {
                let mut fields: HashMap<String, String> = HashMap::new();
                let mut files: Vec<(String, String, Option<String>, usize)> = Vec::new();

                while let Some(field) = multipart.next_field().await.unwrap() {
                    let name = field.name().unwrap().to_string();
                    if let Some(filename) = field.file_name() {
                        let filename = filename.to_string();
                        let content_type = field.content_type().map(|s| s.to_string());
                        let data = field.bytes().await.unwrap();
                        files.push((name, filename, content_type, data.len()));
                    } else {
                        let data = field.text().await.unwrap();
                        fields.insert(name, data);
                    }
                }

                // Return validation result as JSON
                serde_json::json!({
                    "fields": fields,
                    "files_count": files.len(),
                    "file_info": files
                })
                .to_string()
            }),
        );

        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        // Construct client and make request
        let integration = HttpIntegration::new(vec![]);

        // Prepare base64 image
        let dummy_data = b"Hello World Image";
        let engine = base64::engine::general_purpose::STANDARD;
        let b64_data = engine.encode(dummy_data);

        let body_json = serde_json::json!({
            "type": "multipart",
            "fields": [
                {"name": "field1", "value": "value1"},
                {"name": "mode", "value": "test"}
            ],
            "files": [
                {
                    "name": "upfile",
                    "filename": "test.png",
                    "content_type": "image/png",
                    "data_base64": b64_data
                }
            ]
        });

        let args = serde_json::json!({
            "method": "POST",
            "url": format!("http://{}/upload", addr),
            "headers": null,
            "query": null,
            "body": body_json.to_string(),  // Stringify the body
            "timeout_ms": 5000,
            "follow_redirects": false
        });

        let result = integration
            .execute("http_request", &args.to_string())
            .await
            .expect("Request failed");

        let output: Value = serde_json::from_str(&result).unwrap();
        let resp_body: Value = serde_json::from_str(output["body_text"].as_str().unwrap()).unwrap();

        assert_eq!(resp_body["fields"]["field1"], "value1");
        assert_eq!(resp_body["fields"]["mode"], "test");
        assert_eq!(resp_body["files_count"], 1);

        let file_info = resp_body["file_info"].as_array().unwrap()[0]
            .as_array()
            .unwrap();
        assert_eq!(file_info[0], "upfile");
        assert_eq!(file_info[1], "test.png");
        assert_eq!(file_info[2], "image/png");
        assert_eq!(file_info[3], dummy_data.len());
    }

    #[tokio::test]
    async fn test_multipart_size_limit() {
        let integration = HttpIntegration::new(vec![]);

        // Create 600KB dummy data
        let large_data = vec![0u8; 600 * 1024];
        let engine = base64::engine::general_purpose::STANDARD;
        let b64_data = engine.encode(large_data);

        let body_json = serde_json::json!({
            "type": "multipart",
            "fields": [],
            "files": [
                {
                    "name": "too_big",
                    "filename": "big.bin",
                    "data_base64": b64_data
                }
            ]
        });

        let args = serde_json::json!({
            "method": "POST",
            "url": "http://127.0.0.1:1234/upload",
            "headers": null,
            "query": null,
            "body": body_json.to_string(),  // Stringify the body
            "timeout_ms": 1000,
            "follow_redirects": false
        });

        let result = integration.execute("http_request", &args.to_string()).await;
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("exceeds 500KB limit")
        );
    }
}
