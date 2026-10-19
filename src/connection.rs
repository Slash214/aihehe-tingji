use super::{App, AppError, HttpResult, endpoint, speech};
use axum::{Json, extract::State, http::StatusCode};
use serde::Deserialize;
use serde_json::{Value, json};
use std::time::{Duration, Instant};

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Asr,
    Text,
}

#[derive(Deserialize)]
pub struct TestRequest {
    kind: Kind,
    base: String,
    key: String,
    model: String,
    #[serde(default)]
    protocol: String,
    #[serde(default)]
    language: String,
    #[serde(default)]
    verbose: bool,
}

fn invalid(message: &str) -> AppError {
    AppError(StatusCode::BAD_REQUEST, message.into())
}

pub async fn test(
    State(app): State<App>,
    Json(input): Json<TestRequest>,
) -> HttpResult<Json<Value>> {
    if input.key.trim().is_empty() || input.model.trim().is_empty() {
        return Err(invalid("请先填写 API Key 并选择模型"));
    }
    if input.key.len() > 8192 || input.base.len() > 2048 || input.model.len() > 512 {
        return Err(invalid("接口地址、密钥或模型名称过长"));
    }
    if input.language.len() > 8
        || !input
            .language
            .chars()
            .all(|c| c.is_ascii_alphabetic() || c == '-')
    {
        return Err(invalid("语言代码无效"));
    }
    let _permit = app.gate.clone().try_acquire_owned().map_err(|_| {
        AppError(
            StatusCode::CONFLICT,
            "已有任务或测试正在进行，请稍后再试".into(),
        )
    })?;
    let started = Instant::now();
    let is_asr = matches!(input.kind, Kind::Asr);
    if is_asr {
        let settings = speech::Settings {
            base: &input.base,
            key: &input.key,
            model: &input.model,
            protocol: &input.protocol,
            language: &input.language,
            verbose: input.verbose,
        };
        let profile = settings.validate()?;
        speech::transcribe(
            &app.client,
            &settings,
            include_bytes!("../assets/connection-test.wav").to_vec(),
            0.0,
            4.0,
            Duration::from_secs(60),
        )
        .await?;
        return Ok(Json(
            json!({"ok":true,"elapsed_ms":started.elapsed().as_millis(),
            "message":format!("测试通过：{}已完成短音频转写。",profile.label)}),
        ));
    }
    let path = "chat/completions";
    let request = app.client.post(endpoint(&input.base,path)?).bearer_auth(input.key.trim())
        .timeout(Duration::from_secs(60)).json(&json!({
            "model":input.model.trim(),
            "messages":[{"role":"system","content":"这是接口连接测试。请只回复 OK。"},{"role":"user","content":"请回复 OK。"}]
        }));
    let mut response = request
        .send()
        .await
        .map_err(|_| invalid("测试连接失败或超过 60 秒，请检查地址、网络与服务商状态"))?;
    let status = response.status();
    if !status.is_success() {
        let reason = match status.as_u16() {
            401 | 403 => "密钥无效，或没有所选模型的调用权限",
            404 | 405 => "当前地址或模型不支持此文本对话接口，请检查接口地址及模型 ID",
            400 | 422 => "模型或请求格式不兼容，请选择支持 Chat Completions 的文本模型",
            429 => "额度不足或请求过于频繁，请检查余额和限流",
            _ => "服务商返回错误，请稍后重试",
        };
        return Err(invalid(&format!(
            "测试失败 · HTTP {} · /{}：{}",
            status.as_u16(),
            path,
            reason
        )));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| invalid("测试响应读取失败或超时，请重试"))?
    {
        if bytes.len() + chunk.len() > 1024 * 1024 {
            return Err(invalid("测试响应过大，无法确认接口兼容性"));
        }
        bytes.extend_from_slice(&chunk);
    }
    let value: Value = serde_json::from_slice(&bytes)
        .map_err(|_| invalid("接口有响应，但未返回兼容的 JSON，测试未通过"))?;
    {
        let choice = &value["choices"][0];
        if !choice["message"]["content"]
            .as_str()
            .is_some_and(|s| !s.trim().is_empty())
            || choice["finish_reason"]
                .as_str()
                .is_some_and(|s| s != "stop")
        {
            return Err(invalid("文本接口有响应，但未返回完整文字，测试未通过"));
        }
    }
    Ok(Json(
        json!({"ok":true,"elapsed_ms":started.elapsed().as_millis(),
        "message":"测试通过：所选文本模型已返回有效回复。"}),
    ))
}
