use std::fmt;
use std::fmt::Formatter;
use axum::{routing::post, Router, Extension, Json, BoxError, body};
use std::net::SocketAddr;
use anyhow::Result;
use axum::extract::{FromRequest, RequestParts};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use sqlx::mysql::MySqlPoolOptions;
use sqlx::{MySql, Pool, Row, Transaction};
use serde::{Serialize, Deserialize};
use serde::de::DeserializeOwned;
use validator::{Validate};
use thiserror::{Error};
use async_trait::async_trait;
use axum::body::HttpBody;
use sqlx::types::BigDecimal;
use num_traits::{ToPrimitive};

#[tokio::main]
async fn main() -> Result<()> {
    let pool = MySqlPoolOptions::new()
        .max_connections(5)
        .connect("mysql://root@localhost:3306/codetest")
        .await?;

    // database から構文エラーが返ってくる
    // sqlx::query_file!("../db/init.sql").run(&pool).await?;

    let app = Router::new()
        .route("/transactions", post(handler))
        .layer(Extension(pool));

    let addr = SocketAddr::from(([0, 0, 0, 0], 8888));
    axum::Server::bind(&addr)
        .serve(app.into_make_service())
        .await?;
    Ok(())
}

struct ValidatedRequest<T>(T);

// エラー種別
#[derive(Debug, Error)]
enum AppError {
    #[error(transparent)]
    Validation(#[from] validator::ValidationErrors),
    #[error(transparent)]
    JsonRejection(#[from] axum::extract::rejection::JsonRejection),
    #[error(transparent)]
    DBConnection(#[from] sqlx::Error),
    #[error(transparent)]
    DomainSpecification(#[from] DomainSpecificaionError),
}

#[derive(Debug, Error)]
enum DomainSpecificaionError {
    #[error("user {0} has over amount at 1000")]
    UserHasOverAmount(i32),
}

// エラー -> レスポンス実装
impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        match self {
            AppError::Validation(_) => {
                let msg = format!("Validation error: {}", self.to_string());
                (StatusCode::BAD_REQUEST, Json(JSONResponseBody { message: msg }))
            }
            AppError::JsonRejection(_) => (StatusCode::BAD_REQUEST, Json(JSONResponseBody { message: self.to_string() })),
            AppError::DBConnection(_) => {
                let msg = format!("Database error: {}", self.to_string());
                (StatusCode::INTERNAL_SERVER_ERROR, Json(JSONResponseBody { message: msg }))
            }
            AppError::DomainSpecification(_) => {
                (StatusCode::BAD_REQUEST, Json(JSONResponseBody { message: self.to_string() }))
            }
        }.into_response()
    }
}

// JSONリクエストBody -> バリデーション
#[async_trait]
impl<T, B> FromRequest<B> for ValidatedRequest<T>
    where
        T: DeserializeOwned + Validate,
        B: Send + body::HttpBody,
        B::Data: Send,
        B::Error: Into<BoxError>,
{
    type Rejection = AppError;

    async fn from_request(req: &mut RequestParts<B>) -> std::result::Result<Self, Self::Rejection> {
        let Json(value) = Json::<T>::from_request(req).await?;
        value.validate()?;
        Ok(ValidatedRequest(value))
    }
}

#[derive(Debug, Deserialize, Validate)]
struct TransactionRequestBody {
    id: i32,
    user_id: i32,
    #[validate(range(min = 0, max = 1000))]
    amount: i32,
    description: String,
}

// デバッグ用
impl std::fmt::Display for TransactionRequestBody {
    fn fmt(&self, f: &mut fmt::Formatter) -> std::fmt::Result {
        write!(f, "TransactionRequestBody {{ user_id: {}, amount: {}, description: {} }}", self.user_id, self.amount, self.description)
    }
}

#[derive(Debug, Serialize)]
struct JSONResponseBody {
    message: String,
}

async fn handler(Extension(ref pool): Extension<Pool<MySql>>, ValidatedRequest(payload): ValidatedRequest<TransactionRequestBody>) -> Result<impl IntoResponse, AppError> {
    println!("Requested /transactions, body: {}", payload);

    let mut tx: Transaction<MySql> = pool.begin().await?;

    let row: (Option<BigDecimal>, ) = sqlx::query_as("SELECT SUM(amount) as sum FROM transactions WHERE user_id = ? FOR UPDATE")
        .bind(payload.user_id)
        .fetch_one(&mut tx)
        .await?;
    let sum = row.0.unwrap_or_default().to_i64().unwrap_or_default();

    match sum + payload.amount as i64 {
        0..=1000 => {
            sqlx::query("INSERT INTO transactions (user_id, amount, description) values (?, ? ,?)")
                .bind(payload.user_id)
                .bind(payload.amount)
                .bind(payload.description)
                .execute(&mut tx).
                await?;
            tx.commit().await?;
            Ok((StatusCode::OK, Json(JSONResponseBody { message: "Transaction created".to_string() })))
        }
        _ => {
            tx.rollback().await?;
            Err(AppError::DomainSpecification(DomainSpecificaionError::UserHasOverAmount(payload.user_id)))
        }
    }
}