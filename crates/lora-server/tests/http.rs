use std::collections::BTreeMap;
use std::sync::Arc;

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use lora_database::{
    Database, ExecuteOptions, LoraError, LoraErrorCode, LoraValue, QueryPlan, QueryProfile,
    QueryResult, QueryRunner, ResultFormat,
};
use serde_json::json;
use tower::ServiceExt;

use lora_server::{build_app, QueryFormat};

mod http_tests {
    use super::*;

    struct InternalErrorDb;

    impl QueryRunner for InternalErrorDb {
        fn execute(
            &self,
            _query: &str,
            _options: Option<ExecuteOptions>,
        ) -> Result<QueryResult, LoraError> {
            Err(LoraError::new(
                LoraErrorCode::Internal,
                "raw storage detail: /secret/path",
            ))
        }

        fn explain(
            &self,
            _query: &str,
            _params: Option<BTreeMap<String, LoraValue>>,
        ) -> Result<QueryPlan, LoraError> {
            unreachable!("test only exercises /query")
        }

        fn profile(
            &self,
            _query: &str,
            _params: Option<BTreeMap<String, LoraValue>>,
        ) -> Result<QueryProfile, LoraError> {
            unreachable!("test only exercises /query")
        }
    }

    struct ConnectionErrorDb;

    impl QueryRunner for ConnectionErrorDb {
        fn execute(
            &self,
            _query: &str,
            _options: Option<ExecuteOptions>,
        ) -> Result<QueryResult, LoraError> {
            Err(LoraError::new(
                LoraErrorCode::Connection,
                "connect tcp 127.0.0.1:9999: connection refused",
            ))
        }

        fn explain(
            &self,
            _query: &str,
            _params: Option<BTreeMap<String, LoraValue>>,
        ) -> Result<QueryPlan, LoraError> {
            unreachable!("test only exercises /query")
        }

        fn profile(
            &self,
            _query: &str,
            _params: Option<BTreeMap<String, LoraValue>>,
        ) -> Result<QueryProfile, LoraError> {
            unreachable!("test only exercises /query")
        }
    }

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
                    .body(Body::from(
                        json!({ "query": "THIS IS NOT CYPHER" }).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn post_query_malformed_json_returns_structured_error() {
        let db = Arc::new(Database::in_memory());
        let app = build_app(db);

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/query")
                    .header("content-type", "application/json")
                    .body(Body::from("{not json"))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["error"]["code"], "LORA_INVALID_PARAMS");
        assert_eq!(json["error"]["category"], "client");
    }

    #[tokio::test]
    async fn post_query_missing_query_returns_structured_error() {
        let db = Arc::new(Database::in_memory());
        let app = build_app(db);

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/query")
                    .header("content-type", "application/json")
                    .body(Body::from(json!({}).to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["error"]["code"], "LORA_INVALID_PARAMS");
    }

    #[tokio::test]
    async fn post_query_with_params_succeeds() {
        let db = Arc::new(Database::in_memory());
        let app = build_app(db);

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/query")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({
                            "query": "RETURN $v AS v",
                            "format": "rows",
                            "params": { "v": 42 }
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["rows"][0]["v"], 42);
    }

    #[tokio::test]
    async fn post_query_invalid_params_returns_422() {
        let db = Arc::new(Database::in_memory());
        let app = build_app(db);

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/query")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({
                            "query": "RETURN $v AS v",
                            "params": ["not", "an", "object"]
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["error"]["code"], "LORA_INVALID_PARAMS");
    }

    #[tokio::test]
    async fn post_query_unique_constraint_returns_409() {
        let db = Arc::new(Database::in_memory());
        let app = build_app(db);

        for query in [
            "CREATE CONSTRAINT user_id FOR (u:User) REQUIRE u.id IS UNIQUE",
            "CREATE (u:User {id: 1}) RETURN u",
        ] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri("/query")
                        .header("content-type", "application/json")
                        .body(Body::from(json!({ "query": query }).to_string()))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
        }

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/query")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({ "query": "CREATE (u:User {id: 1}) RETURN u" }).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::CONFLICT);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["error"]["code"], "LORA_UNIQUE_CONSTRAINT");
        assert_eq!(json["error"]["category"], "client");
        assert!(json["error"]["message"]
            .as_str()
            .unwrap()
            .contains("uniqueness"));
    }

    #[tokio::test]
    async fn explain_invalid_params_returns_422() {
        let db = Arc::new(Database::in_memory());
        let app = build_app(db);

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/explain")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({
                            "query": "RETURN $x",
                            "params": ["not", "an", "object"]
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["error"]["code"], "LORA_INVALID_PARAMS");
        assert_eq!(
            json["error"]["message"],
            "params must be an object keyed by parameter name"
        );
    }

    #[tokio::test]
    async fn post_query_internal_error_is_sanitized() {
        let db = Arc::new(InternalErrorDb);
        let app = build_app(db);

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/query")
                    .header("content-type", "application/json")
                    .body(Body::from(json!({ "query": "RETURN 1" }).to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["error"]["code"], "LORA_INTERNAL");
        assert_eq!(
            json["error"]["message"],
            "database operation failed unexpectedly"
        );
        assert!(!json["error"]["message"]
            .as_str()
            .unwrap()
            .contains("/secret/path"));
    }

    #[tokio::test]
    async fn post_query_connection_error_returns_503_and_sanitizes_detail() {
        let db = Arc::new(ConnectionErrorDb);
        let app = build_app(db);

        let (status, json) = post_query(app, "RETURN 1").await;

        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(json["error"]["code"], "LORA_CONNECTION");
        assert_eq!(
            json["error"]["message"],
            "database connection is temporarily unavailable"
        );
        assert!(!json["error"]["message"]
            .as_str()
            .unwrap()
            .contains("127.0.0.1"));
    }

    #[tokio::test]
    async fn post_query_not_null_constraint_returns_409() {
        let db = Arc::new(Database::in_memory());
        let app = build_app(Arc::clone(&db));

        let _ = post_query(
            app.clone(),
            "CREATE CONSTRAINT author_name FOR (a:Author) REQUIRE a.name IS NOT NULL",
        )
        .await;
        let (status, json) = post_query(app, "CREATE (:Author)").await;

        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(json["error"]["code"], "LORA_NOT_NULL_CONSTRAINT");
    }

    #[tokio::test]
    async fn post_query_missing_catalog_entry_returns_404() {
        let db = Arc::new(Database::in_memory());
        let app = build_app(db);

        let (status, json) = post_query(app, "DROP CONSTRAINT missing").await;

        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(json["error"]["code"], "LORA_NOT_FOUND");
    }

    #[tokio::test]
    async fn post_query_foreign_key_violation_returns_409() {
        let db = Arc::new(Database::in_memory());
        let app = build_app(Arc::clone(&db));

        let _ = post_query(app.clone(), "CREATE (:A {id: 1})-[:R]->(:B {id: 2})").await;
        let (status, json) = post_query(app, "MATCH (n:A) DELETE n").await;

        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(json["error"]["code"], "LORA_FOREIGN_KEY");
    }

    #[tokio::test]
    async fn post_query_validation_error_returns_422() {
        let db = Arc::new(Database::in_memory());
        let app = build_app(db);

        let (status, json) = post_query(
            app,
            "CREATE VECTOR INDEX bad FOR (n:Movie) ON (n.embedding) \
             OPTIONS {indexConfig: {`vector.dimensions`: 0, `vector.similarity_function`: 'cosine'}}",
        )
        .await;

        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(json["error"]["code"], "LORA_VALIDATION");
    }

    #[test]
    fn query_format_rows_maps_to_result_format_rows() {
        assert!(matches!(
            ResultFormat::from(QueryFormat::Rows),
            ResultFormat::Rows
        ));
    }

    async fn post_query(app: axum::Router, query: &str) -> (StatusCode, serde_json::Value) {
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/query")
                    .header("content-type", "application/json")
                    .body(Body::from(json!({ "query": query }).to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json = if bytes.is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap()
        };
        (status, json)
    }
}
