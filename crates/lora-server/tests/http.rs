use std::sync::Arc;

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use lora_database::{Database, ResultFormat};
use serde_json::json;
use tower::ServiceExt;

use lora_server::{build_app, QueryFormat};

mod http_tests {
    use super::*;

    #[tokio::test]
    async fn get_health_returns_200_ok() {
        let db = Arc::new(Database::in_memory());
        let app = build_app(db);

        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn parse_valid_lora_succeeds() {
        let db = Database::in_memory();

        assert!(db
            .parse("CREATE (n:User {id: 1, name: 'alice'}) RETURN n")
            .is_ok());
    }

    #[tokio::test]
    async fn parse_invalid_lora_fails() {
        let db = Database::in_memory();

        assert!(db.parse("THIS IS NOT CYPHER").is_err());
    }

    #[tokio::test]
    async fn post_query_create_then_match_returns_200() {
        let db = Arc::new(Database::in_memory());
        let app = build_app(Arc::clone(&db));

        let _ = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/query")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({
                            "query": "CREATE (n:User {id: 1, name: 'alice'}) RETURN n"
                        })
                            .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/query")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({
                            "query": "MATCH (n:User {id: 1}) RETURN n"
                        })
                            .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn post_query_invalid_lora_returns_400() {
        let db = Arc::new(Database::in_memory());
        let app = build_app(db);

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/query")
                    .header("content-type", "application/json")
                    .body(Body::from(json!({ "query": "THIS IS NOT CYPHER" }).to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn query_format_rows_maps_to_result_format_rows() {
        assert!(matches!(
            ResultFormat::from(QueryFormat::Rows),
            ResultFormat::Rows
        ));
    }
}
