use std::{fmt};
use std::borrow::Cow;
use std::thread;
use axum::{routing::post, Router, Extension, Json, BoxError, body};
use std::net::SocketAddr;
use std::time::Duration;
use anyhow::Result;
use axum::extract::{FromRequest, RequestParts};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use sqlx::mysql::{MySqlPoolOptions};
use sqlx::{MySql, Pool, Transaction};
use serde::{Serialize, Deserialize};
use serde::de::DeserializeOwned;
use validator::{Validate};
use thiserror::{Error};
use async_trait::async_trait;
use num_traits::{ToPrimitive};
use sqlx::types::BigDecimal;

#[tokio::main]
async fn main() -> Result<()> {
    let db_host = std::env::var("DB_HOST")?;
    // DBプール生成
    let pool = MySqlPoolOptions::new()
        .max_connections(5)
        .connect(format!("mysql://root@{}:3306/codetest", db_host).as_str())
        .await?;

    // database から構文エラーが返ってくる
    // sqlx::query_file!("../db/init.sql").run(&pool).await?;

    // ルーター作成
    let app = Router::new()
        .route("/transactions", post(handler))
        .layer(Extension(pool));

    let addr = SocketAddr::from(([0, 0, 0, 0], 8888));
    // アプリケーション作成とServe
    axum::Server::bind(&addr)
        .serve(app.into_make_service())
        .await?;

    Ok(())
}

// Tをバリデーション済みの型
// axum の Handler を通る必要がある
struct ValidatedRequest<T>(T);

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

// ドメイン仕様エラー
// Application層でドメインそのもののエラーとして扱う
#[derive(Debug, Error)]
enum DomainSpecificaionError {
    #[error("user {0} has over amount at 1000")]
    UserHasOverAmount(i32),
}

// ライブラリ側に実装がないので作る
enum MySqlErrorCode {
    Unknown,
    DeadlockFound,
}

// &str -> MySqlErrorCode
impl<'a> From<Cow<'a, str>> for MySqlErrorCode {
    fn from(str: Cow<'a, str>) -> Self {
        match str.as_ref() {
            "40001" => {
                MySqlErrorCode::DeadlockFound
            }
            _ => {
                MySqlErrorCode::Unknown
            }
        }
    }
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

// POST /transactions HTTPBody
#[derive(Deserialize, Validate)]
struct TransactionRequestBody {
    user_id: i32,
    #[validate(range(min = 1, max = 1000))]
    amount: i32,
    description: String,
}

// デバッグ用
impl std::fmt::Display for TransactionRequestBody {
    fn fmt(&self, f: &mut fmt::Formatter) -> std::fmt::Result {
        write!(f, "TransactionRequestBody {{ user_id: {}, amount: {}, description: {} }}", self.user_id, self.amount, self.description)
    }
}

// レスポンス汎用型
// exp. Ok(StatusCode.OK, Json(JSONResponseBody))
#[derive(Debug, Serialize)]
struct JSONResponseBody {
    message: String,
}

// POST /transactions Handler
async fn handler(Extension(ref pool): Extension<Pool<MySql>>, ValidatedRequest(payload): ValidatedRequest<TransactionRequestBody>) -> Result<impl IntoResponse, AppError> {
    println!("Requested /transactions, body: {}", payload);

    // トランザクション復帰遅延 ms
    let tx_retry_duration = Duration::from_millis(50);

    // トランザクションエラーから復帰のため loop で処理を行う
    loop {
        // トランザクション開始
        let mut tx: Transaction<MySql> = pool.begin().await?;
        // user_id=payload.user_id の現在のamount合計を取得する
        #[derive(sqlx::FromRow)]
        struct SumQueryResult {
            sum: Option<BigDecimal>,
        }
        let res = sqlx::query_as::<_, SumQueryResult>("SELECT SUM(amount) as sum FROM transactions WHERE user_id = ? FOR UPDATE")
            .bind(payload.user_id)
            .fetch_one(&mut tx).await;

        match res {
            // amount取得正常終了
            Ok(row) => {
                // BigDecimalは扱いづらく、アプリケーションの使用上MAX1000までにしかならないためi64に変換する
                let current_amount = row.sum.unwrap_or_default().to_i64().unwrap_or_default();
                // 現在の合計値 + リクエストのamount
                match current_amount + payload.amount as i64 {
                    // 0~1000まで
                    0..=1000 => {
                        // INSERT クエリ実行
                        let insert_res = sqlx::query(
                            "INSERT INTO transactions (user_id, amount, description) VALUES (?, ?, ?)"
                        )
                            .bind(payload.user_id)
                            .bind(payload.amount)
                            .bind(payload.description.clone())
                            .execute(&mut tx)
                            .await;
                        match insert_res {
                            // クエリ完了
                            Ok(_) => {
                                tx.commit().await?;
                                break Ok((StatusCode::OK, Json(JSONResponseBody { message: "Transaction created".to_string() })));
                            }
                            // DBエラーが出た場合
                            Err(sqlx::Error::Database(db_error)) => {
                                match db_error.code().map(MySqlErrorCode::from).unwrap_or(MySqlErrorCode::Unknown) {
                                    // Deadlockエラーが出た時は
                                    MySqlErrorCode::DeadlockFound => {
                                        // 遅延
                                        thread::sleep(tx_retry_duration);
                                        // ロールバック
                                        tx.rollback().await?;
                                        // 続行
                                        continue;
                                    }
                                    // 謎のエラーが出た時は終了
                                    MySqlErrorCode::Unknown => {
                                        break Err(AppError::DBConnection(sqlx::Error::Database(db_error)));
                                    }
                                }
                            }
                            // その他のエラーが出た時は終了
                            Err(e) => {
                                tx.rollback().await?;
                                break Err(AppError::DBConnection(e));
                            }
                        }
                    }
                    // 1001以上
                    _ => {
                        tx.rollback().await?;
                        break Err(AppError::DomainSpecification(DomainSpecificaionError::UserHasOverAmount(payload.user_id)));
                    }
                }
            }
            // DBエラーが出た場合
            Err(sqlx::Error::Database(db_error)) => {
                match db_error.
                    code()
                    .map(MySqlErrorCode::from)
                    .unwrap_or(MySqlErrorCode::Unknown)
                {
                    // Deadlockが発生した時
                    MySqlErrorCode::DeadlockFound => {
                        // 遅延して
                        thread::sleep(tx_retry_duration);
                        // ロールバック
                        tx.rollback().await?;
                        // ループ続行
                        continue;
                    }
                    // 謎のエラーが出た時は終了
                    MySqlErrorCode::Unknown => {
                        break Err(AppError::DBConnection(sqlx::Error::Database(db_error)));
                    }
                }
            }
            // その他のエラーが出た時は終了
            Err(e) => {
                tx.rollback().await?;
                break Err(AppError::DBConnection(e));
            }
        }
    }
}
