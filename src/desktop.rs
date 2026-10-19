use crate::{Context, Result, data_root};
use std::{rc::Rc, sync::mpsc, time::Duration};
use tao::{
    dpi::LogicalSize,
    event::{Event, WindowEvent},
    event_loop::{ControlFlow, EventLoopBuilder},
    window::{Icon, ResizeDirection, Theme, WindowBuilder},
};
use wry::{NewWindowResponse, WebContext, WebViewBuilder};

pub fn run(lock: std::fs::File) -> Result<()> {
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let outcome = tokio::runtime::Runtime::new()
            .map_err(anyhow::Error::from)
            .and_then(|runtime| runtime.block_on(crate::serve(Some(sender))));
        if let Err(e) = outcome {
            let _ = std::fs::write(data_root().join("startup-error.log"), format!("{e:#}"));
        }
    });
    let (port, gate) = receiver
        .recv_timeout(Duration::from_secs(20))
        .context("本地引擎启动失败，请检查端口是否占用，详见 data/startup-error.log")?;
    let event_loop = EventLoopBuilder::<String>::with_user_event().build();
    let window = Rc::new(
        WindowBuilder::new()
            .with_title("爱呵呵 · 听记")
            .with_inner_size(LogicalSize::new(1220.0, 820.0))
            .with_min_inner_size(LogicalSize::new(900.0, 650.0))
            .with_decorations(false)
            .with_theme(Some(Theme::Dark))
            .with_window_icon(
                Icon::from_rgba(include_bytes!("../assets/icon.rgba").to_vec(), 64, 64).ok(),
            )
            .build(&event_loop)?,
    );
    let root = data_root();
    let mut context = WebContext::new(Some(root.join("webview")));
    let proxy = event_loop.create_proxy();
    let download_proxy = event_loop.create_proxy();
    let download_parent = window.clone();
    let url = format!("http://127.0.0.1:{port}/");
    let allowed_url = url.clone();
    let webview = WebViewBuilder::new_with_web_context(&mut context)
        .with_url(url)
        .with_background_color((18, 18, 22, 255))
        .with_devtools(false)
        .with_initialization_script("window.AIHEHE_DESKTOP = true;")
        .with_ipc_handler(move |request| {
            let _ = proxy.send_event(request.body().clone());
        })
        .with_navigation_handler(move |target| {
            target == allowed_url || target.starts_with(&format!("blob:{allowed_url}"))
        })
        .with_new_window_req_handler(|_, _| NewWindowResponse::Deny)
        .with_download_started_handler(move |_, destination| {
            let file_name = destination
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("知识文档.md");
            let dialog = rfd::FileDialog::new()
                .set_parent(download_parent.as_ref())
                .set_title("导出文档")
                .set_file_name(file_name);
            if let Some(path) = dialog.save_file() {
                *destination = path;
                true
            } else {
                false
            }
        })
        .with_download_completed_handler(move |_, path, success| {
            let message = if success {
                format!(
                    "文档已导出：{}",
                    path.map(|p| p.display().to_string()).unwrap_or_default()
                )
            } else {
                "导出未完成或已取消".into()
            };
            let _ = download_proxy.send_event(format!("toast:{message}"));
        })
        .build(window.as_ref())
        .context("无法创建桌面窗口，请检查 Microsoft Edge WebView2 Runtime 是否已安装")?;
    window.set_focus();
    event_loop.run(move |event, _, flow| {
        // Keep these alive for the entire GUI process.
        let _keep_alive = (&lock, &context);
        *flow = ControlFlow::Wait;
        let may_close = || {
            gate.available_permits() > 0
                || rfd::MessageDialog::new()
                    .set_parent(window.as_ref())
                    .set_title("任务仍在处理中")
                    .set_description(
                        "退出会中断当前任务，已完成的部分会保留。下次打开可以继续。确定退出？",
                    )
                    .set_buttons(rfd::MessageButtons::YesNo)
                    .show()
                    == rfd::MessageDialogResult::Yes
        };
        match event {
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => {
                if may_close() {
                    *flow = ControlFlow::Exit;
                }
            }
            Event::UserEvent(message) => match message.as_str() {
                "close" => {
                    if may_close() {
                        *flow = ControlFlow::Exit;
                    }
                }
                "minimize" => window.set_minimized(true),
                "maximize" => window.set_maximized(!window.is_maximized()),
                "drag" => {
                    let _ = window.drag_window();
                }
                "resize" => {
                    let _ = window.drag_resize_window(ResizeDirection::SouthEast);
                }
                "open-data" => {
                    let _ = std::process::Command::new("explorer.exe")
                        .arg(&root)
                        .spawn();
                }
                _ => {
                    if let Some(text) = message.strip_prefix("toast:") {
                        let escaped = serde_json::to_string(text).unwrap_or_default();
                        let _ = webview.evaluate_script(&format!("toast({escaped})"));
                    }
                }
            },
            _ => {}
        }
    });
}
