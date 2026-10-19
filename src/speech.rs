use super::{App, HttpResult, Segment, endpoint, parse_segments};
use anyhow::{Context, Result, bail, ensure};
use axum::{Json, extract::State};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::time::Duration;

const DASH_PATH: &str = "api/v1/services/aigc/multimodal-generation/generation";

#[derive(Deserialize)]
pub struct ProfileRequest {
    pub base: String,
    pub model: String,
    #[serde(default)]
    pub protocol: String,
}

#[derive(Clone, Serialize)]
pub struct Profile {
    pub protocol: String,
    pub label: String,
    pub hint: String,
    pub chunk_seconds: u32,
    pub timestamps: bool,
    #[serde(skip)]
    pub url: String,
}

fn family(model: &str, name: &str) -> bool {
    model == name
        || model.strip_prefix(name).is_some_and(|suffix| {
            suffix.starts_with('-')
                && suffix[1..].chars().all(|c| c.is_ascii_digit() || c == '-')
                && suffix[1..].chars().any(|c| c.is_ascii_digit())
        })
}

fn aliyun(base: &str) -> bool {
    reqwest::Url::parse(base.trim())
        .ok()
        .and_then(|u| u.host_str().map(str::to_owned))
        .is_some_and(|h| {
            h == "dashscope.aliyuncs.com"
                || h == "dashscope-intl.aliyuncs.com"
                || h == "dashscope-us.aliyuncs.com"
                || h.ends_with(".maas.aliyuncs.com")
        })
}

fn native_url(base: &str) -> Result<String> {
    endpoint(base, "check")?;
    let clean = base.trim().trim_end_matches('/');
    if clean.ends_with(DASH_PATH) {
        return Ok(clean.into());
    }
    let root = clean
        .strip_suffix("/compatible-mode/v1")
        .or_else(|| clean.strip_suffix("/api/v1"))
        .or_else(|| clean.strip_suffix("/v1"))
        .unwrap_or(clean);
    endpoint(root, DASH_PATH)
}

fn chat_url(base: &str) -> Result<String> {
    endpoint(base, "check")?;
    let clean = base.trim().trim_end_matches('/');
    if clean.ends_with("/chat/completions") {
        return Ok(clean.into());
    }
    if let Some(root) = clean.strip_suffix("/api/v1") {
        return endpoint(root, "compatible-mode/v1/chat/completions");
    }
    if reqwest::Url::parse(clean)?
        .path()
        .trim_matches('/')
        .is_empty()
        && aliyun(base)
    {
        return endpoint(clean, "compatible-mode/v1/chat/completions");
    }
    endpoint(clean, "chat/completions")
}

pub fn resolve(base: &str, model: &str, selected: &str) -> Result<Profile> {
    ensure!(
        base.len() <= 2048 && model.len() <= 512,
        "接口地址或模型名称过长"
    );
    endpoint(base, "check")?;
    let model = model.trim().to_lowercase();
    ensure!(!model.is_empty(), "请先选择语音模型");
    let is_ali = aliyun(base);
    let auto = selected.is_empty() || selected == "auto";
    let unsupported = model.contains("filetrans")
        || model.contains("realtime")
        || model.contains("streaming")
        || model.ends_with("-message")
        || model.starts_with("paraformer")
        || model == "fun-asr";
    if unsupported && (auto || is_ali || selected != "openai") {
        bail!(
            "该模型需要实时或异步转写协议，当前版本尚未适配。请选择 qwen-audio-3.0-asr-flash、qwen3-asr-flash 或支持通用转写接口的模型"
        );
    }
    let dash = [
        "qwen-audio-3.0-asr-flash",
        "qwen-audio-3.1-asr-flash",
        "fun-asr-flash",
    ]
    .iter()
    .any(|name| family(&model, name));
    let qwen = family(&model, "qwen3-asr-flash");
    let protocol = if auto {
        if is_ali && dash {
            "dashscope"
        } else if is_ali && qwen {
            "qwen_chat"
        } else if is_ali {
            bail!(
                "此阿里云模型尚未自动适配语音转写。请选择已支持的语音模型，或按服务商文档手动指定接口类型"
            );
        } else if dash || qwen {
            bail!("检测到千问语音模型，但当前为自定义服务地址。请按服务商文档选择接口类型后测试");
        } else {
            "openai"
        }
    } else {
        selected
    };
    let (label, hint, chunk_seconds, timestamps, url) = match protocol {
        "openai" => (
            "通用音频转写",
            "使用通用文件转写接口。自定义模型需通过测试确认可用。",
            300,
            true,
            endpoint(base, "audio/transcriptions")?,
        ),
        "dashscope" => (
            "阿里云语音 · DashScope",
            "使用阿里云原生语音接口，每段约 3 分钟；时间位置标注音频段范围。",
            180,
            false,
            native_url(base)?,
        ),
        "qwen_chat" => (
            "千问语音 · 音频消息",
            "通过音频消息调用 Qwen3-ASR，每段约 3 分钟；时间位置标注音频段范围。",
            180,
            false,
            chat_url(base)?,
        ),
        _ => bail!("不支持的语音接口类型"),
    };
    Ok(Profile {
        protocol: protocol.into(),
        label: label.into(),
        hint: hint.into(),
        chunk_seconds,
        timestamps,
        url,
    })
}

pub async fn profile(
    State(_app): State<App>,
    Json(input): Json<ProfileRequest>,
) -> HttpResult<Json<Profile>> {
    Ok(Json(resolve(&input.base, &input.model, &input.protocol)?))
}

pub struct Settings<'a> {
    pub base: &'a str,
    pub key: &'a str,
    pub model: &'a str,
    pub protocol: &'a str,
    pub language: &'a str,
    pub verbose: bool,
}

impl Settings<'_> {
    pub fn validate(&self) -> Result<Profile> {
        ensure!(
            !self.key.trim().is_empty() && self.key.len() <= 8192,
            "请填写有效的语音 API Key"
        );
        ensure!(
            self.language.len() <= 8
                && self
                    .language
                    .chars()
                    .all(|c| c.is_ascii_alphabetic() || c == '-'),
            "语言代码无效"
        );
        let profile = resolve(self.base, self.model, self.protocol)?;
        ensure!(
            profile.timestamps || !self.verbose,
            "当前语音接口不支持 verbose_json 逐句时间戳，请关闭该选项；仍会保留音频段时间位置"
        );
        Ok(profile)
    }
}

fn normalize(profile: &Profile, value: Value) -> Result<Value> {
    ensure!(
        value.get("code").is_none() && value.get("error").is_none(),
        "语音服务返回业务错误，请检查模型权限、额度及参数"
    );
    match profile.protocol.as_str() {
        "dashscope" => {
            // output.sentence can be only the last sentence. Prefer the full output.text.
            let text = value["output"]["text"]
                .as_str()
                .filter(|s| !s.trim().is_empty())
                .context("阿里云语音响应缺少完整 output.text，无法确认转写完整性")?;
            Ok(json!({"text":text}))
        }
        "qwen_chat" => {
            let choice = &value["choices"][0];
            ensure!(
                choice["finish_reason"].as_str().is_none_or(|s| s == "stop"),
                "千问语音返回未完成，已保留此前转写"
            );
            let text = choice["message"]["content"]
                .as_str()
                .filter(|s| !s.trim().is_empty())
                .context("千问语音接口未返回有效转写文字")?;
            Ok(json!({"text":text}))
        }
        _ => Ok(value),
    }
}

pub async fn transcribe(
    client: &reqwest::Client,
    settings: &Settings<'_>,
    bytes: Vec<u8>,
    offset: f64,
    seconds: f64,
    timeout: Duration,
) -> Result<Vec<Segment>> {
    let profile = settings.validate()?;
    let request = client
        .post(&profile.url)
        .bearer_auth(settings.key.trim())
        .timeout(timeout);
    let request = if profile.protocol == "openai" {
        let mut form = reqwest::multipart::Form::new()
            .text("model", settings.model.trim().to_owned())
            .text(
                "response_format",
                if settings.verbose {
                    "verbose_json"
                } else {
                    "json"
                },
            )
            .part(
                "file",
                reqwest::multipart::Part::bytes(bytes)
                    .file_name("audio.wav")
                    .mime_str("audio/wav")?,
            );
        if !settings.language.is_empty() {
            form = form.text("language", settings.language.to_owned());
        }
        request.multipart(form)
    } else {
        ensure!(
            seconds <= 300.0 && bytes.len() <= 10_000_000,
            "当前语音接口单段音频超出限制，请重新导入素材"
        );
        let audio = format!("data:audio/wav;base64,{}", STANDARD.encode(bytes));
        let content = json!([{"type":"input_audio","input_audio":{"data":audio}}]);
        if profile.protocol == "dashscope" {
            let mut parameters = json!({"format":"wav","sample_rate":"16000"});
            if !settings.language.is_empty() {
                parameters["language_hints"] = json!([settings.language]);
            }
            request.header("X-DashScope-SSE","disable").json(&json!({
                "model":settings.model.trim(),"input":{"messages":[{"role":"user","content":content}]},"parameters":parameters
            }))
        } else {
            let mut options = json!({"enable_itn":false});
            if !settings.language.is_empty() {
                options["language"] = json!(settings.language);
            }
            request.json(&json!({"model":settings.model.trim(),"messages":[{"role":"user","content":content}],
                "stream":false,"asr_options":options}))
        }
    };
    let mut response = request.send().await.context("语音接口连接失败或超时")?;
    if !response.status().is_success() {
        let status = response.status().as_u16();
        let hint = match status {
            401 | 403 => "密钥无效或没有所选模型权限",
            404 | 405 => "地址、模型或接口类型不匹配；获取模型列表不代表支持此转写协议",
            400 | 422 => "模型或参数不兼容，请检查接口类型、语言和时间戳设置",
            429 => "额度不足或请求过于频繁",
            _ => "服务商返回错误，请稍后重试",
        };
        let path = reqwest::Url::parse(&profile.url)?.path().to_owned();
        bail!("语音测试/转写失败 · HTTP {status} · {path}：{hint}");
    }
    let mut data = Vec::new();
    while let Some(chunk) = response.chunk().await.context("语音响应读取失败或超时")? {
        ensure!(data.len() + chunk.len() <= 4 * 1024 * 1024, "语音响应过大");
        data.extend_from_slice(&chunk);
    }
    let value: Value = serde_json::from_slice(&data).context("语音接口未返回兼容的 JSON")?;
    parse_segments(&normalize(&profile, value)?, offset, seconds)
        .context("语音接口未返回有效完整转写，测试未通过")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn routes_preserve_host_and_reject_unsupported_families() {
        let base = "https://ws-example.cn-beijing.maas.aliyuncs.com/compatible-mode/v1";
        let p = resolve(base, "qwen-audio-3.0-asr-flash", "auto").unwrap();
        assert_eq!(
            p.url,
            "https://ws-example.cn-beijing.maas.aliyuncs.com/api/v1/services/aigc/multimodal-generation/generation"
        );
        assert_eq!(p.chunk_seconds, 180);
        assert!(!p.timestamps);
        assert_eq!(
            resolve(base, "qwen3-asr-flash", "auto").unwrap().protocol,
            "qwen_chat"
        );
        for name in [
            "qwen-audio-3.0-asr-flash-filetrans",
            "qwen3-asr-flash-realtime",
            "paraformer-v2",
            "deepseek-v4",
        ] {
            assert!(resolve(base, name, "auto").is_err());
        }
        assert!(
            resolve(
                "https://proxy.example/v1",
                "qwen-audio-3.0-asr-flash",
                "auto"
            )
            .is_err()
        );
        assert_eq!(
            resolve(
                "https://proxy.example/v1",
                "qwen-audio-3.0-asr-flash",
                "openai"
            )
            .unwrap()
            .protocol,
            "openai"
        );
        assert!(resolve("https://key@api.example/v1", "whisper-1", "auto").is_err());
    }
    #[test]
    fn full_text_wins_over_last_sentence() {
        let p = resolve("http://localhost/v1", "mock", "dashscope").unwrap();
        let v = normalize(
            &p,
            json!({"output":{"text":"第一句。第二句。","sentence":{"text":"第二句。"}}}),
        )
        .unwrap();
        assert_eq!(v["text"], "第一句。第二句。");
        assert!(normalize(&p, json!({"code":"InvalidApiKey","message":"secret"})).is_err());
        assert!(normalize(&p, json!({"output":{"sentence":{"text":"最后一句"}}})).is_err());
    }
}
