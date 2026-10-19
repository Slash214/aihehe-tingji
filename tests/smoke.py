"""Local integration test, no paid APIs.
Run after cargo build: python tests/smoke.py [--exe PATH] [--ffmpeg-dir PATH]
"""
import argparse
import base64
import io
import json
import os
from pathlib import Path
import re
import socket
import subprocess
import tempfile
import threading
import time
import urllib.error
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import wave


class Mock(BaseHTTPRequestHandler):
    counts = {"asr": 0, "chat": 0}
    fail_chat = True
    verbose = False
    truncate = False
    last_post = None
    speech_calls = []

    def check_audio_message(self, payload, native):
        content = payload["input"]["messages"][0]["content"] if native else payload["messages"][0]["content"]
        assert content[0]["type"] == "input_audio"
        uri = content[0]["input_audio"]["data"]
        assert uri.startswith("data:audio/wav;base64,")
        audio = base64.b64decode(uri.split(",",1)[1], validate=True)
        assert len(audio) <= 10_000_000
        with wave.open(io.BytesIO(audio)) as wav:
            assert wav.getframerate() == 16000 and wav.getnchannels() == 1
            seconds = wav.getnframes() / wav.getframerate()
            assert seconds <= 300
        if native:
            assert payload["parameters"]["format"] == "wav"
            assert payload["parameters"]["sample_rate"] == "16000"
            assert self.headers["X-DashScope-SSE"] == "disable"
        else:
            assert payload["stream"] is False and payload["asr_options"]["enable_itn"] is False
        Mock.speech_calls.append((native,seconds,payload))

    def log_message(self, *_):
        pass

    def do_GET(self):
        if self.headers.get("Authorization") != "Bearer mock-only-secret":
            self.send_error(401, "mock-only-secret")
            return
        if self.path == "/empty/v1/models":
            result = {"data": []}
        elif self.path == "/bad/v1/models":
            result = {"unexpected": True}
        elif self.path in ["/v1/models", "/catalog-only/v1/models"]:
            result = {"data": [{"id": "mock-text"}, {"id": "mock-asr"}, {"id": "mock-text"}, {}]}
        else:
            self.send_error(404)
            return
        data = json.dumps(result).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_POST(self):
        body = self.rfile.read(int(self.headers["Content-Length"]))
        Mock.last_post = (self.path, body)
        if self.headers.get("Authorization") != "Bearer mock-only-secret":
            self.send_error(401)
            return
        status = 200
        if self.path.startswith("/bad-response/"):
            result = {"unexpected": True, "debug": "mock-only-secret"}
        elif self.path.startswith("/empty-response/"):
            result = {"text": "", "choices":[{"finish_reason":"stop","message":{"content":""}}]}
        elif self.path.startswith("/rate-limit/"):
            status, result = 429, {"error": "mock-only-secret"}
        elif self.path.endswith("/api/v1/services/aigc/multimodal-generation/generation"):
            payload = json.loads(body)
            self.check_audio_message(payload, True)
            if payload["model"] == "business-error":
                result = {"code":"InvalidApiKey","message":"mock-only-secret"}
            else:
                result = {"output":{"text":"第一句完整内容。第二句完整内容。","sentence":{"text":"第二句完整内容。","begin_time":500,"end_time":900}}}
        elif self.path == "/v1/audio/transcriptions":
            Mock.counts["asr"] += 1
            assert b'filename="audio.wav"' in body and b'RIFF' in body
            if Mock.verbose:
                result = {"text": "带时间戳的测试", "segments": [{"start": 0, "end": 0.5, "text": "带时间戳的测试"}]}
            else:
                result = {"text": "第一步记录案例，第二步总结知识。" * 650 + "尾部保留标记"}
        elif self.path == "/v1/chat/completions" and json.loads(body)["model"] == "qwen3-asr-flash":
            payload = json.loads(body)
            self.check_audio_message(payload, False)
            result = {"choices":[{"finish_reason":"stop","message":{"content":"千问语音完整内容。"}}]}
        elif self.path == "/v1/chat/completions":
            Mock.counts["chat"] += 1
            assert json.loads(body)["model"] == "mock-text"
            if Mock.fail_chat:
                status = 503
                result = {"error": "mock-only-secret MUST NOT appear in app error"}
            else:
                result = {"choices": [{"finish_reason": "length" if Mock.truncate else "stop", "message": {"content": "### 核心观点\n\n记录案例，再总结知识。[00:00:00]\n\n<script>不得执行</script>"}}]}
        else:
            self.send_error(404)
            return
        data = json.dumps(result, ensure_ascii=False).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)


def main():
    parser = argparse.ArgumentParser()
    root = Path(__file__).resolve().parents[1]
    parser.add_argument("--exe", default=str(root / "target/debug/aihehe-transcriber.exe"))
    parser.add_argument("--ffmpeg-dir", default=str(root / "tools"))
    args = parser.parse_args()
    provider = ThreadingHTTPServer(("127.0.0.1", 0), Mock)
    threading.Thread(target=provider.serve_forever, daemon=True).start()
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        port = sock.getsockname()[1]
    base = f"http://127.0.0.1:{port}"
    token = ""
    process = None

    def request(path, data=None, headers=None, auth=True):
        h = {"x-app-token": token} if auth else {}
        h.update(headers or {})
        req = urllib.request.Request(base + path, data=data, headers=h)
        with urllib.request.urlopen(req, timeout=20) as response:
            return response.read()

    def wait_job(job_id):
        deadline = time.monotonic() + 60
        while time.monotonic() < deadline:
            job = json.loads(request("/api/jobs/" + job_id))
            if job["status"] != "running":
                return job
            time.sleep(0.1)
        raise AssertionError("job timeout")

    def upload(config, data, filename, video=None):
        boundary = "AIHEHE_TEST_BOUNDARY"
        body = (f'--{boundary}\r\nContent-Disposition: form-data; name="config"\r\n\r\n'.encode()
                + json.dumps(config).encode()
                + f'\r\n--{boundary}\r\nContent-Disposition: form-data; name="file"; filename="{filename}"\r\nContent-Type: application/octet-stream\r\n\r\n'.encode()
                + data)
        if video:
            body += (f'\r\n--{boundary}\r\nContent-Disposition: form-data; name="video"; filename="{video[0]}"\r\nContent-Type: video/mp4\r\n\r\n'.encode() + video[1])
        body += f"\r\n--{boundary}--\r\n".encode()
        return json.loads(request("/api/jobs", body, {"Content-Type": f"multipart/form-data; boundary={boundary}"}))["id"]

    def boot(folder):
        nonlocal token, process
        env = os.environ.copy()
        env.update(AIHEHE_DATA=str(folder / "data"), AIHEHE_PORT=str(port), AIHEHE_FFMPEG_DIR=args.ffmpeg_dir)
        process = subprocess.Popen([args.exe, "--serve"], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                                   creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0))
        for _ in range(100):
            try:
                html = request("/", auth=False).decode()
                token = re.search(r'const token="([^"]+)"', html).group(1)
                return
            except (OSError, AttributeError):
                if process.poll() is not None:
                    raise AssertionError("server exited")
                time.sleep(0.1)
        raise AssertionError("server startup timeout")

    try:
        with tempfile.TemporaryDirectory(prefix="aihehe-smoke-") as folder_name:
            folder = Path(folder_name)
            boot(folder)
            health = json.loads(request("/api/health"))
            assert health["ffmpeg"] and health["ffprobe"]
            model_base = f"http://127.0.0.1:{provider.server_port}"
            def get_models(path="/v1", key="mock-only-secret"):
                return json.loads(request("/api/models", json.dumps({"base": model_base + path, "key":key}).encode(), {"Content-Type":"application/json"}))
            assert get_models()["models"] == ["mock-asr", "mock-text"]
            assert get_models("/empty/v1")["models"] == []
            for path, key in [("/v1", "invalid"), ("/missing/v1", "mock-only-secret"), ("/bad/v1", "mock-only-secret"), ("/v1", "")]:
                try:
                    get_models(path,key)
                    raise AssertionError("invalid model request accepted")
                except urllib.error.HTTPError as e:
                    assert e.code == 400
                    assert b"mock-only-secret" not in e.read()
            assert request("/style.css") and request("/models.js")
            print("PASS: model listing, deduplication, empty catalog, invalid key, unsupported endpoint, schema error, no credential echo")
            def probe(kind, path="/v1", key="mock-only-secret", **extra):
                payload = dict(kind=kind, base=model_base + path, key=key, model="mock-asr" if kind=="asr" else "mock-text")
                payload.update(extra)
                return json.loads(request("/api/test-connection", json.dumps(payload).encode(), {"Content-Type":"application/json"}))
            assert probe("asr", language="zh", verbose=True)["ok"]
            assert Mock.last_post[0] == "/v1/audio/transcriptions"
            assert b"verbose_json" in Mock.last_post[1] and b"zh" in Mock.last_post[1] and b"RIFF" in Mock.last_post[1]
            Mock.fail_chat = False
            assert probe("text")["ok"]
            assert Mock.last_post[0] == "/v1/chat/completions"
            assert json.loads(Mock.last_post[1])["messages"][0]["role"] == "system"
            assert get_models("/catalog-only/v1")["models"]
            for kind, path, key, expected in [
                ("asr", "/catalog-only/v1", "mock-only-secret", "/audio/transcriptions"),
                ("text", "/catalog-only/v1", "mock-only-secret", "/chat/completions"),
                ("asr", "/v1", "invalid", "401"),
                ("text", "/rate-limit/v1", "mock-only-secret", "429"),
                ("asr", "/bad-response/v1", "mock-only-secret", "未通过"),
                ("text", "/bad-response/v1", "mock-only-secret", "未通过"),
                ("asr", "/empty-response/v1", "mock-only-secret", "未通过"),
                ("text", "/empty-response/v1", "mock-only-secret", "未通过"),
            ]:
                try:
                    probe(kind, path, key)
                    raise AssertionError("incompatible endpoint incorrectly passed")
                except urllib.error.HTTPError as e:
                    detail = e.read().decode()
                    assert e.code == 400 and expected in detail and "mock-only-secret" not in detail, detail
            Mock.truncate = True
            try:
                probe("text")
                raise AssertionError("incomplete text passed")
            except urllib.error.HTTPError as e:
                assert "未通过" in e.read().decode()
            Mock.truncate, Mock.fail_chat = False, True
            Mock.counts = {"asr":0, "chat":0}
            print("PASS: real endpoint probes, ASR parameters and audio, chat response, catalog-success/transcription-404, auth, rate limit, invalid/empty/truncated responses")
            try:
                request("/api/jobs", auth=False)
                raise AssertionError("unauthenticated API accepted")
            except urllib.error.HTTPError as e:
                assert e.code == 403
            try:
                request("/", headers={"Host": "attacker.example"})
                raise AssertionError("invalid Host accepted")
            except urllib.error.HTTPError as e:
                assert e.code == 403
            config = {
                "asr_base": f"http://127.0.0.1:{provider.server_port}/v1", "asr_key": "mock-only-secret",
                "asr_model": "mock-asr", "language": "zh", "verbose": False, "organize": True,
                "text_base": f"http://127.0.0.1:{provider.server_port}/v1",
                "text_key": "mock-only-secret", "text_model": "mock-text"
            }
            wav = folder / "long.wav"
            with wave.open(str(wav), "wb") as stream:
                stream.setnchannels(1)
                stream.setsampwidth(2)
                stream.setframerate(16000)
                stream.writeframes(b"\0\0" * 16000 * 301)
            job_id = upload(config, wav.read_bytes(), "测试录音.wav")
            job = wait_job(job_id)
            assert job["status"] == "failed", job
            assert len(job["segments"]) == 2, job["segments"]
            assert job["segments"][1]["start"] >= 299.9
            assert "HTTP 503" in job["error"] and "mock-only-secret" not in job["error"]
            assert Mock.counts["asr"] == 2
            exported = request(f"/api/jobs/{job_id}/download/transcript.md").decode()
            assert exported.count("尾部保留标记") == 2
            assert job["segments"][0]["precision"] == "chunk"
            Mock.fail_chat = False
            request(f"/api/jobs/{job_id}/retry", json.dumps(config).encode(), {"Content-Type": "application/json"})
            job = wait_job(job_id)
            assert job["status"] == "done", job.get("error")
            assert Mock.counts["asr"] == 2, "retry re-billed cached ASR"
            assert "第 3 章" in job["knowledge"], "long transcript wasn't split"
            for kind in ["transcript.md", "transcript.txt", "knowledge.md", "segments.json"]:
                assert request(f"/api/jobs/{job_id}/download/{kind}")
            for file in (folder / "data").rglob("*"):
                if file.suffix in [".json", ".md", ".tmp"]:
                    assert b"mock-only-secret" not in file.read_bytes(), f"key leaked in {file}"
            print("PASS: long audio, real FFmpeg split, timing, long notes, exports, recovery, cached ASR, no key persistence")
            process.terminate()
            process.wait(timeout=10)
            boot(folder)
            restored = json.loads(request("/api/jobs/" + job_id))
            assert restored["status"] == "done" and restored["knowledge"]
            print("PASS: restart restores completed jobs")
            bad_id = upload(config, b"not media", "broken.mp4")
            bad = wait_job(bad_id)
            assert bad["status"] == "failed" and "媒体" in bad["error"]
            assert Mock.counts["asr"] == 2
            print("PASS: corrupt media fails before calling paid API")
            Mock.verbose = True
            with wave.open(str(wav), "wb") as stream:
                stream.setnchannels(1)
                stream.setsampwidth(2)
                stream.setframerate(16000)
                stream.writeframes(b"\0\0" * 16000)
            config["verbose"] = True
            config["organize"] = False
            short_id = upload(config, wav.read_bytes(), "短音频.wav")
            short = wait_job(short_id)
            assert short["status"] == "done" and short["segments"][0]["precision"] == "segment"
            Mock.truncate = True
            config["organize"] = True
            request(f"/api/jobs/{short_id}/retry", json.dumps(config).encode(), {"Content-Type": "application/json"})
            short = wait_job(short_id)
            assert short["status"] == "failed" and "未完成" in short["error"]
            assert short["segments"] and Mock.counts["asr"] == 3
            print("PASS: verbose timestamps, transcription-only, truncated LLM output rejected with original retained")
            silent = folder / "silent.mp4"
            subprocess.run([str(Path(args.ffmpeg_dir) / "ffmpeg.exe"), "-v", "error", "-f", "lavfi", "-i", "color=c=black:s=32x32:d=1", "-an", "-c:v", "mpeg4", str(silent)], check=True, creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0))
            config.update(organize=False, input_kind="video")
            silent_id = upload(config, silent.read_bytes(), "无音轨视频.mp4")
            silent_job = wait_job(silent_id)
            assert silent_job["status"] == "failed" and "没有音轨" in silent_job["error"]
            assert Mock.counts["asr"] == 3, "silent video called paid API"
            config["input_kind"] = "audio"
            paired_id = upload(config, wav.read_bytes(), "独立录音.wav", ("无音轨视频.mp4", silent.read_bytes()))
            paired = wait_job(paired_id)
            assert paired["status"] == "done", paired.get("error")
            assert paired["audio_source"] == "独立录音.wav" and paired["video_source"] == "无音轨视频.mp4"
            assert Mock.counts["asr"] == 4
            assert (folder / "data" / paired_id / "video.media").read_bytes() == silent.read_bytes()
            paired_doc = request(f"/api/jobs/{paired_id}/download/transcript.md").decode()
            assert "独立音频" in paired_doc and "未与视频自动对齐" in paired_doc
            print("PASS: silent video rejected before API, paired video plus audio uses independent audio, sources retained")
            def profile(model, protocol="auto", address="https://ws-test.cn-beijing.maas.aliyuncs.com/compatible-mode/v1"):
                return json.loads(request("/api/asr-profile", json.dumps(dict(base=address, model=model, protocol=protocol)).encode(), {"Content-Type":"application/json"}))
            assert profile("qwen-audio-3.0-asr-flash")["protocol"] == "dashscope"
            assert profile("fun-asr-flash-2026-06-15")["chunk_seconds"] == 180
            assert profile("qwen3-asr-flash")["protocol"] == "qwen_chat"
            for model in ["qwen-audio-3.0-asr-flash-filetrans","qwen3-asr-flash-realtime","paraformer-v2","deepseek-v4"]:
                try:
                    profile(model)
                    raise AssertionError("unsupported model auto accepted")
                except urllib.error.HTTPError as e:
                    assert e.code == 400
            assert probe("asr", protocol="dashscope", model="qwen-audio-3.0-asr-flash",language="zh")["ok"]
            assert Mock.speech_calls[-1][2]["parameters"]["language_hints"] == ["zh"]
            assert probe("asr", protocol="qwen_chat", model="qwen3-asr-flash",language="en")["ok"]
            assert Mock.speech_calls[-1][2]["asr_options"]["language"] == "en"
            for opts in [dict(protocol="dashscope",model="business-error"),dict(protocol="dashscope",verbose=True)]:
                try:
                    probe("asr", **opts)
                    raise AssertionError("business error or unsupported timestamps passed")
                except urllib.error.HTTPError as e:
                    assert e.code == 400 and b"mock-only-secret" not in e.read()
            with wave.open(str(wav), "wb") as stream:
                stream.setnchannels(1)
                stream.setsampwidth(2)
                stream.setframerate(16000)
                stream.writeframes(b"\0\0" * 16000 * 361)
            ali_config = dict(config, asr_base=model_base+"/v1", asr_protocol="dashscope",asr_model="qwen-audio-3.0-asr-flash",verbose=False,organize=False)
            before = len(Mock.speech_calls)
            ali_id = upload(ali_config, wav.read_bytes(), "阿里长音频.wav")
            ali = wait_job(ali_id)
            assert ali["status"] == "done", ali.get("error")
            assert ali["asr_protocol"] == "dashscope" and len(ali["segments"]) == 3
            assert 179 < ali["segments"][1]["start"] < 181 and 359 < ali["segments"][2]["start"] < 361
            assert len(Mock.speech_calls) == before+3
            assert all(call[1] < 181 for call in Mock.speech_calls[before:])
            assert all("第一句完整内容" in seg["text"] and seg["precision"]=="chunk" for seg in ali["segments"])
            request(f"/api/jobs/{ali_id}/retry",json.dumps(ali_config).encode(),{"Content-Type":"application/json"})
            assert wait_job(ali_id)["status"] == "done" and len(Mock.speech_calls) == before+3
            try:
                request(f"/api/jobs/{ali_id}/retry",json.dumps(dict(ali_config,asr_protocol="openai")).encode(),{"Content-Type":"application/json"})
                raise AssertionError("cached audio reused with different protocol")
            except urllib.error.HTTPError as e:
                assert e.code == 400
            old_config = dict(ali_config, asr_protocol="openai",asr_base=model_base+"/catalog-only/v1")
            old_id = upload(old_config, wav.read_bytes(), "旧接口失败后恢复.wav")
            assert wait_job(old_id)["status"] == "failed"
            request(f"/api/jobs/{old_id}/retry",json.dumps(dict(old_config,asr_protocol="dashscope")).encode(),{"Content-Type":"application/json"})
            recovered = wait_job(old_id)
            assert recovered["status"] == "done" and len(recovered["segments"]) == 3
            qwen_config = dict(ali_config,asr_protocol="qwen_chat",asr_model="qwen3-asr-flash")
            qwen_id = upload(qwen_config, wav.read_bytes(), "千问长音频.wav")
            qwen = wait_job(qwen_id)
            assert qwen["status"] == "done" and len(qwen["segments"]) == 3, qwen.get("error")
            assert qwen["segments"][2]["text"] == "千问语音完整内容。"
            process.terminate()
            process.wait(timeout=10)
            boot(folder)
            assert json.loads(request("/api/jobs/"+ali_id))["asr_protocol"] == "dashscope"
            print("PASS: native DashScope and Qwen audio messages, shared probe/job payloads, 3-minute chunks, full text retained, retry cache isolation, old failed job recovery, persisted protocol")
            print("ALL SMOKE CHECKS PASSED")
    finally:
        if process and process.poll() is None:
            process.terminate()
            process.wait(timeout=10)
        provider.shutdown()


if __name__ == "__main__":
    main()
