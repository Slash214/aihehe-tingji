$ErrorActionPreference = 'Stop'
Push-Location $PSScriptRoot
try {
    cargo build --release --locked
    if ($LASTEXITCODE -ne 0) { throw 'Rust 编译失败。' }
    Copy-Item -LiteralPath 'target\release\aihehe-transcriber.exe' -Destination '.\爱呵呵听记.exe' -Force
    Write-Host '编译完成。双击 爱呵呵听记.exe 启动。'
} finally { Pop-Location }
