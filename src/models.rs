use super::{App, AppError, HttpResult, endpoint};
use axum::{Json, extract::State, http::StatusCode};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{collections::BTreeSet, time::Duration};

#[derive(Deserialize)]
pub struct ModelRequest {
    base: String,
    key: String,
}

fn invalid(message: &str) -> AppError {
    AppError(StatusCode::BAD_REQUEST, message.into())
}

fn model_ids(value: &Value) -> anyhow::Result<Vec<String>> {
    let entries = value
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("接口没有返回标准模型列表，可手动填写模型 ID"))?;
    let ids: BTreeSet<String> = entries
        .iter()
        .filter_map(|entry| entry.get("id").and_then(Value::as_str))
        .filter(|id| !id.trim().is_empty() && id.len() <= 512 && !id.chars().any(char::is_control))
        .map(str::to_owned)
        .collect();
    Ok(ids.into_iter().collect())
}

pub async fn list(
    State(app): State<App>,
    Json(input): Json<ModelRequest>,
) -> HttpResult<Json<Value>> {
    if input.key.trim().is_empty() {
        return Err(invalid("请先填写 API Key"));
    }
    if input.key.len() > 8192 || input.base.len() > 2048 {
        return Err(invalid("接口地址或密钥过长"));
    }
    let url = endpoint(&input.base, "models")?;
    let mut response = app
        .client
        .get(url)
        .bearer_auth(input.key.trim())
        .timeout(Duration::from_secs(20))
        .send()
        .await
        .map_err(|_| invalid("连接失败或超时，请检查接口地址与网络"))?;
    if !response.status().is_success() {
        let message = match response.status().as_u16() {
            401 | 403 => "密钥无效或没有模型列表权限，请检查 Key",
            404 | 405 => "此接口不支持获取模型列表，请检查地址或手动填写模型 ID",
            429 => "请求过于频繁或额度不足，请稍后重试",
            _ => "服务商暂时无法提供模型列表，请稍后重试或手动填写",
        };
        return Err(invalid(message));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| invalid("读取模型列表失败，请重试"))?
    {
        if bytes.len() + chunk.len() > 2 * 1024 * 1024 {
            return Err(invalid("模型列表过大，请手动填写模型 ID"));
        }
        bytes.extend_from_slice(&chunk);
    }
    let value: Value = serde_json::from_slice(&bytes)
        .map_err(|_| invalid("接口未返回 JSON 模型列表，请检查地址或手动填写"))?;
    let models = model_ids(&value)?;
    Ok(Json(json!({"models":models})))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn models_are_sorted_deduplicated_and_validated() {
        let result = model_ids(&json!({"data":[{"id":"z"},{"id":"a"},{"id":"z"},{"id":""},{"id":5},{"id":"bad\nname"},{}]})).unwrap();
        assert_eq!(result, vec!["a", "z"]);
        assert!(model_ids(&json!({"data":[]})).unwrap().is_empty());
        assert!(model_ids(&json!({"error":"not a model list"})).is_err());
    }
}
