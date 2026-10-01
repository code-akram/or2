## Voice dictation for or2 without Google services: research report

### Findings

| Option | Code / weights licence | Size | Speed (measured on a Snapdragon 870 unless noted) | Streaming | Rust / Android |
|---|---|---|---|---|---|
| **sherpa-onnx** (runtime) | Apache-2.0 | n/a | n/a | Yes, for streaming model families | Official `sherpa-onnx` crate. Android arm64 and static linking supported. The old `sherpa-rs` is deprecated. |
| **Parakeet TDT 0.6B v3 int8** (25 European languages) | CC-BY-4.0 weights | about 640 MB | RTF 0.59 sustained, WER 4.97% clean / 9.34% noisy, final text 0–350 ms after the end of speech | Not true streaming. sherpa-onnx's Android build "simulates" it with VAD (voice-activity-detection) segments. | Runs via sherpa-onnx |
| **Parakeet 110M** | CC-BY-4.0 weights (~440 MB checkpoint) | small | Not found | No | Via parakeet.cpp (MIT, ggml) or sherpa-onnx |
| **Nemotron Speech Streaming 0.6B** | NVIDIA Open Model License (permits redistribution with attribution; I did not review it against GPL) | about 0.6B parameters | Not found | Yes, cache-aware | Exports exist for sherpa-onnx, still new |
| **Moonshine v2 streaming** (tiny 34M, small 123M, medium 245M) | MIT code. English streaming models are MIT. Legacy non-English models are non-commercial. | Not found | Medium-en: RTF 0.78, WER 4.71% clean / 7.02% noisy. Pi 5 latency: tiny 237 ms, small 527 ms. | Yes, native | The C API has no Rust bindings. It runs through ONNX Runtime. sherpa-onnx also supports Moonshine. |
| **whisper.cpp / whisper-rs** | MIT; Unlicense for whisper-rs; MIT weights | base/small | Whisper small: RTF 1.35, WER 5.31% clean / 14.43% noisy, final text takes 2.7 s | No | whisper-rs was archived on GitHub in July 2025 and moved to Codeberg |
| **Vosk** | Apache-2.0 | about 50 MB small models | Not found | Yes | Kaldi-based. Accuracy is clearly behind the others, and I did not verify per-model licences. |
| **Android `SpeechRecognizer`** | Platform API | 0 MB | English WER 9.72% clean / 27.09% noisy | Yes | See below |

- **Snapdragon 8 Gen 1 numbers are an inference.** I found no 8 Gen 1 benchmark. The only like-for-like data is the SD870 (Poco F3) bench, which tested all of these models with the same harness. The 8 Gen 1 should be faster, so treat the RTFs above as an upper bound. Confirm on the owner's phone before committing.
- **Moonshine versus Parakeet.**
  - Moonshine small or medium streaming gives live partials with low latency. That fits a hold-to-talk composer.
  - Parakeet v3 is more accurate and multilingual, but 640 MB is large and it is not streaming.
- **`SpeechRecognizer` is a poor fit.**
  - The default engine is Google's. `createOnDeviceSpeechRecognizer` (API 33+) only works if a local engine is installed, and `isOnDeviceRecognitionAvailable()` tells you whether one is.
  - FUTO Voice Input does not implement the `SpeechRecognizer` API; its own page says that is only planned.
  - On de-Googled ROMs there is usually nothing behind the API.
  - Its quality was the worst in the bench, especially with noise.
- **BYOK API shape.**
  - OpenAI's `/v1/audio/transcriptions` accepts a file up to 25 MB (mp3, m4a, wav, webm and others).
  - It supports `prompt` and `keywords` for context, and `stream=true` for delta events.
  - `gpt-transcribe` is the current recommended model, and `whisper-1` is legacy.
  - Many servers (Groq, faster-whisper-server, whisper.cpp server) copy this endpoint, so supporting the OpenAI shape covers most of them. Moshi's v3.13.0 BYOK is also an "OpenAI-compatible endpoint".
- **Moshi's setup.** It ships whisper.rn and parakeet.cpp, adds Parakeet 110M, multilingual Parakeet and Nemotron as separate options, and offers hold-to-dictate on the toolbar mic, word replacements, and replay/retry of recordings. Its Pro tier raises a cloud dictation cap from 3 min to 60 min.

### Recommended design

Two engines behind one small `Transcriber` interface in a new Rust `or2-voice` crate. Kotlin owns `AudioRecord` and the permission prompt and streams 16 kHz mono PCM to Rust.

1. **BYOK endpoint (M1, effort S–M, high impact, ship first).**
   - The user sets a base URL, a model name and a key.
   - The key is kept Keystore-wrapped on the Android side and passed per call, so Rust still has no storage.
   - There is no default endpoint and the feature is off by default.
   - It records on hold, then POSTs a WAV or Opus file on release, so a 25 MB cap is not a concern for short clips.
   - Offer "your own host" as an endpoint. If a whisper server runs on the owner's machine, open a `direct-tcpip` channel over the existing SSH connection. That keeps audio off third-party clouds and uses the `Transport` trait, so no new socket rules are needed.
   - For a third-party HTTPS endpoint, either add an HTTPS transport implementation or document an explicit exception in `docs/design.md`. AGENTS.md says everything network goes through `Transport` and Kotlin never speaks a wire protocol, so do not quietly add an HTTP call in Kotlin.
2. **On-device (M2, effort L, high impact, privacy-preserving default).**
   - Use sherpa-onnx's official Rust crate with Moonshine small streaming as the default model.
   - Offer Parakeet v3 int8 as an optional "high accuracy / multilingual" download.
   - Models are not bundled. They are downloaded on explicit user action, with the URL and SHA-256 pinned in the app.
   - Silero VAD handles segmentation.
3. **UI.** Hold the mic to record, release to transcribe, and insert the result into the composer rather than straight into the PTY, so the owner reviews it before sending to an agent. Add a Kotlin-side word-replacement map (S effort) like Moshi's, and a technical-terms hint via `prompt` for BYOK.
4. **Skip** whisper.cpp (slow, no streaming, and sherpa-onnx already runs Whisper if wanted), Vosk (accuracy), Nemotron (new, with a custom licence), and `SpeechRecognizer`. At most, offer the system IME's own mic as a free fallback; this does not apply to the Canvas terminal itself.

### Licence and F-Droid implications

- **GPL-3.0 compatibility.** sherpa-onnx (Apache-2.0), ONNX Runtime (MIT), Silero VAD (MIT) and Moonshine (MIT) are compatible with GPL-3.0-or-later. Parakeet's CC-BY-4.0 weights are data and not linked code, but they need attribution in `THIRD_PARTY_NOTICES.md`. Record each model's source, pinned commit, licence and copyright line.
- **Prebuilt binaries.** The sherpa-onnx Rust crate's `build.rs` downloads a prebuilt `-lib` archive from GitHub releases when `SHERPA_ONNX_LIB_DIR` is not set. F-Droid will not accept that. The F-Droid recipe must build sherpa-onnx and ONNX Runtime from source for arm64 and point `SHERPA_ONNX_LIB_DIR` at the result. That build is slow and fiddly.
- **Model downloads.**
  - F-Droid bans downloading executables without consent. It has no formal ML-model policy, and a maintainer said a non-free model is covered by the NonFreeAsset anti-feature.
  - CC-BY and MIT weights are free, but they cannot be reproduced from training data, which is a grey area.
  - Not bundling the weights keeps the APK clean.
- **Anti-features.** A BYOK feature to a user-supplied endpoint may still draw a NonFreeNet flag, and that needs a maintainer's answer. Keep it opt-in, with no pre-filled vendor URL.
- **APK size.** Statically linked ONNX Runtime will add several MB to the arm64 library. I have no measurement.

### Risks

- The 8 Gen 1 RTF is unmeasured. Parakeet v3 at 640 MB may be too heavy for RAM and storage on the owner's phone.
- sherpa-onnx's Rust crate is young. Android cross-compilation PRs are still landing (see PR #3991), so pin it to a commit.
- Moonshine's licences vary by language. Only ship models whose licence file you have read.
- BYOK sends audio to a third party. It needs a clear consent screen, and the key must never be logged.
- Audio permission and foreground-service rules apply while recording on Android 14+.
- Streaming Parakeet and Nemotron support in sherpa-onnx is immature. Do not plan on it.

### Sources

- [On-device streaming ASR bench, Snapdragon 870](https://github.com/davamix/ondevice-streaming-asr-bench)
- [sherpa-onnx](https://github.com/k2-fsa/sherpa-onnx)
- [sherpa-onnx crate docs](https://docs.rs/sherpa-onnx)
- [sherpa-rs (deprecated)](https://github.com/thewh1teagle/sherpa-rs)
- [sherpa-onnx PR #3991](https://github.com/k2-fsa/sherpa-onnx/pull/3991)
- [sherpa-onnx issue #2918 (Parakeet streaming)](https://github.com/k2-fsa/sherpa-onnx/issues/2918)
- [NeMo models in sherpa-onnx](https://k2-fsa.github.io/sherpa/onnx/pretrained_models/offline-transducer/nemo-transducer-models.html)
- [Moonshine](https://github.com/moonshine-ai/moonshine)
- [Moonshine v2 paper](https://arxiv.org/html/2602.12241v1)
- [mudler/parakeet.cpp](https://github.com/mudler/parakeet.cpp)
- [Nemotron Speech Streaming](https://huggingface.co/nvidia/nemotron-speech-streaming-en-0.6b)
- [whisper-rs](https://github.com/tazz4843/whisper-rs)
- [Vosk models](https://alphacephei.com/vosk/models)
- [FUTO Voice Input mirror](https://github.com/lrq3000/futo-voiceinput-whisper)
- [Android `SpeechRecognizer` reference](https://developer.android.com/reference/android/speech/SpeechRecognizer)
- [OpenAI speech-to-text guide](https://developers.openai.com/api/docs/guides/speech-to-text)
- [F-Droid inclusion policy](https://f-droid.org/en/docs/Inclusion_Policy/)
- [F-Droid forum: libre AI policy](https://forum.f-droid.org/t/does-f-droid-have-a-formal-policy-on-libre-ai/33279)
- Local files: `/home/akram/code/or2/docs/design.md`, `/home/akram/code/or2/.amp/in/moshi/63-whatsnew-all.txt`, `/home/akram/code/or2/.amp/in/moshi/64-research-inputs.md`, `/home/akram/code/or2/.amp/in/moshi/60-licenses.txt`. `docs/roadmap.md` does not exist; the roadmap is inside `design.md`, where M4 lists voice.