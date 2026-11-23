# Speech-to-text engines for voice mode

Bead `wheel-e1xk.1`. It measures three on-device engines (WhisperKit, FluidAudio's Parakeet and whisper.cpp) against one fixture of spoken commands, so voice mode (`docs/voice-mode/index.md` §2) can pick one per platform.

**State:** the Mac is measured. The iPhone is not: the bench app is built and installed, but the phone has to be unlocked to run it. whisper.cpp has no iPhone build yet, because the xcframework packaging step fails under Turnstile (below).

## Fixture

`stt-bench/fixture/utterances.tsv` holds 24 orchestrator-style commands with developer terms ("cargo test", "wheeld", "wheelhouse", "GPUI", "rebase", "npm install"). `stt-bench/fixture/make.sh` renders each one with one of six system voices (Samantha, Daniel, Karen, Moira, Rishi, Flo), as 16 kHz mono WAV with 0.3 s of lead-in and 0.6 s of tail. It writes two sets:

- **clean**
- **noisy**, mixed with pink noise at about 10 dB SNR (speech −21.6 dB RMS, noise −32.0 dB RMS)

The voices are synthetic, which flatters every engine equally. Real speech is not in this fixture.

**Metrics:**
- **WER:** word error rate after lowercasing, removing punctuation and spelling digits as words. A split compound ("work tree" for "worktree", "Fluid Audio" for "FluidAudio") counts as an error for every engine.
- **Terms:** the share of each clip's key terms that appear verbatim.
- **Latency:** wall time from handing over the finished clip to getting text back. For a batch engine, that is the delay after end of speech.
- **Peak MB:** the process's peak physical footprint (`task_vm_info`). Memory the Neural Engine holds is not in this number. For whisper.cpp it is the server's RSS.

## Mac results

The Mac is an M3 Max on macOS 26.6.2, using WhisperKit 1.1.1, FluidAudio 0.18.4 and whisper.cpp 1.9.2 (Homebrew `whisper-server`, Metal, flash attention, 8 threads). Each engine ran in its own process with its models already downloaded, one engine after another, between 04:00 and 04:03 UTC (11:00–11:03pm EST). Other agents' builds were running, at load average 6–12 on 14 cores.

| Engine | Set | WER % | Terms % | Median ms | p90 ms | Peak MB |
|---|---|---|---|---|---|---|
| Parakeet TDT 0.6b v3 (FluidAudio) | clean | 7.5 | 92 | 92 | 148 | 66 |
| Parakeet TDT 0.6b v3 (FluidAudio) | noisy | 15.5 | 75 | 92 | 131 | 66 |
| Parakeet TDT 0.6b v2 (FluidAudio) | clean | 8.9 | 87 | 58 | 64 | 68 |
| Parakeet TDT 0.6b v2 (FluidAudio) | noisy | 17.4 | 68 | 57 | 86 | 68 |
| Whisper small.en 217MB (WhisperKit) | clean | 9.9 | 83 | 359 | 508 | 272 |
| Whisper small.en 217MB (WhisperKit) | noisy | 20.2 | 66 | 305 | 365 | 272 |
| Whisper large-v3 turbo 632MB (WhisperKit) | clean | 8.5 | 89 | 494 | 541 | 257 |
| Whisper large-v3 turbo 632MB (WhisperKit) | noisy | 14.6 | 72 | 496 | 567 | 257 |
| Whisper small.en (whisper.cpp) | clean | 11.3 | 79 | 199 | 297 | 712 |
| Whisper small.en (whisper.cpp) | noisy | 21.6 | 62 | 194 | 228 | 712 |
| Whisper large-v3 turbo q5_0 (whisper.cpp) | clean | 7.5 | 89 | 793 | 867 | 755 |
| Whisper large-v3 turbo q5_0 (whisper.cpp) | noisy | 17.8 | 68 | 811 | 997 | 755 |

Raw transcripts are in `stt-bench/results/*.json`.

**Load time** depends on the Core ML compile cache. The first run after download took 34–51 s (Core ML compiles for the Neural Engine). A run right after it took 0.3–3.4 s. The run in the table took 23–26 s for Parakeet and WhisperKit small, so the compiled cache did not always survive between processes. Voice mode should load the model when the app starts or when the voice button appears, not when the person taps it.

What every engine got wrong in the clean set:
- "wheeld" came out as "wield" or "wheeled".
- "SwiftUI" came out as "Swift O" from Parakeet.
- "llama broker" came out as "Lama Broker" from Whisper.

## Mac pick: Parakeet TDT 0.6b v3 through FluidAudio

- Its clean WER ties for lowest (7.5%), it has the best term recall (92%), and it returns text 92 ms after the clip ends. WhisperKit large-v3 turbo takes 494 ms; whisper.cpp large takes 793 ms.
- In noise, WhisperKit large-v3 turbo is slightly better (14.6% vs 15.5% WER). That gap is smaller than the difference in delay.
- FluidAudio also ships Silero VAD and Parakeet EOU (streaming with end-of-utterance detection), which turn-taking needs (`wheel-e1xk.13`), and Kokoro, which `wheel-e1xk.4` tests. One package covers all three.
- Parakeet v2 (English only) is faster (58 ms) but less accurate on developer terms (87%).

## iPhone

The bench app (`stt-bench/BenchApp`, bundle `com.piercemakes.sttbench`) is installed on an iPhone 16. To measure:
1. Unlock the phone, plug it in and keep the screen on.
2. Run `bash docs/voice-mode/stt-bench/run-iphone.sh`. It runs each engine twice (download and compile, then measure) and pulls the JSON into `stt-bench/results/iphone/`.
3. Score it with `python3 stt-bench/score.py stt-bench/fixture/utterances.tsv stt-bench/results/iphone/*.pass2.json`.

FluidAudio's `parakeet-redux` (about 220 MB, iOS 18+) is the smaller Parakeet to add if v3's 469 MB is too large for the phone.

Battery drain is not measured. A phone on a cable charges, so it needs a wireless run of a 10-minute voice session.

whisper.cpp on the iPhone: `build-xcframework.sh` builds every slice, but its last step, `xcodebuild -create-xcframework`, exits with `error: invalid argument '-jobs'`. Turnstile appends `-jobs 4` to that call, both through its shim and through `turnstile run`, although `turnstile classify` reports the command as pass through.

## What each engine downloads (ADR-0013)

| Engine | Model | From | On disk |
|---|---|---|---|
| FluidAudio | Parakeet TDT 0.6b v3 | `https://huggingface.co/FluidInference/parakeet-tdt-0.6b-v3-coreml` | 469 MB |
| FluidAudio | Parakeet TDT 0.6b v2 | `https://huggingface.co/FluidInference/parakeet-tdt-0.6b-v2-coreml` | 451 MB |
| WhisperKit | `openai_whisper-large-v3-v20240930_turbo_632MB` | `https://huggingface.co/argmaxinc/whisperkit-coreml` | 624 MB |
| WhisperKit | `openai_whisper-small.en_217MB` | `https://huggingface.co/argmaxinc/whisperkit-coreml` | 211 MB |
| whisper.cpp | `ggml-large-v3-turbo-q5_0.bin` | `https://huggingface.co/ggerganov/whisper.cpp` | 547 MB |
| whisper.cpp | `ggml-small.en.bin` | `https://huggingface.co/ggerganov/whisper.cpp` | 465 MB |

FluidAudio caches models in `~/Library/Application Support/FluidAudio/Models`. WhisperKit caches them under its `downloadBase`, which defaults to `~/Documents/huggingface`, so Wheel must set it to its own cache folder.
