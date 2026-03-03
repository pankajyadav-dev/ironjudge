use axum::{Json, response::IntoResponse, http::StatusCode, extract::Path};
use uuid::Uuid;
use types_lib::*;


pub async fn health()-> Result<impl IntoResponse, (StatusCode, String)>{
    Ok((StatusCode::OK, Json("the service is healthy")))
}
pub async fn test_post(
    Json(payload): Json<TaskPayload>
) -> Result<impl IntoResponse,(StatusCode,String)> {
    println!("Received Task: {:?}", payload);
    let random_id = Uuid::new_v4().to_string();
    let response = SubmissionIdPayload::success(random_id);
    Ok((StatusCode::OK, Json(response)))
}
pub async fn run_post(
    Json(payload): Json<TaskPayload>
) -> Result<impl IntoResponse,(StatusCode,String)> {
    println!("Received Task: {:?}", payload);
    let random_id = Uuid::new_v4().to_string();
    let response = SubmissionIdPayload::success(random_id);
    Ok((StatusCode::OK, Json(response)))
}
pub async fn status_get(
    Path(id): Path<String>
) -> Result<impl IntoResponse,(StatusCode,Json<ResponsePayload>)> {
    println!("Received Status Request: {:?}", id);
    let response = ResponsePayload::processing();
    Ok((StatusCode::OK, Json(response)))
}