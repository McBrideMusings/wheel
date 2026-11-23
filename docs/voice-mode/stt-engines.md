# Speech-to-text engines for voice mode

Bead `wheel-e1xk.1`. It measures three on-device engines (WhisperKit, FluidAudio's Parakeet and whisper.cpp) against one fixture of spoken commands, so voice mode (`docs/voice-mode/index.md` §2) can pick one per platform.

**Pick: Parakeet TDT 0.6b v3 through FluidAudio, on the Mac and the iPhone.** Both devices are measured for WhisperKit and Parakeet. whisper.cpp is measured on the Mac only, because its iPhone xcframework packaging step fails under Turnstile (below).

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

## Why Parakeet v3

- Its clean WER ties for lowest (7.5%), it has the best term recall (92%), and it returns text 92 ms after the clip ends. WhisperKit large-v3 turbo takes 494 ms; whisper.cpp large takes 793 ms.
- In noise, WhisperKit large-v3 turbo is slightly better (14.6% vs 15.5% WER). That gap is smaller than the difference in delay.
- FluidAudio also ships Silero VAD and Parakeet EOU (streaming with end-of-utterance detection), which turn-taking needs (`wheel-e1xk.13`), and Kokoro, which `wheel-e1xk.4` tests. One package covers all three.
- Parakeet v2 (English only) is faster (58 ms) but less accurate on developer terms (87%).

## iPhone results

The phone is an iPhone 16 (iPhone17,3), on a cable, on battery charge between 75% and 65%, with thermal state 0 (nominal) throughout. The run was 04:37–05:19 UTC on 23 Nov (11:37pm–12:19am EST). Each engine ran twice in the STT Bench app (`stt-bench/BenchApp`): pass 1 downloads and compiles, pass 2 is measured. The table is pass 2; raw output is in `stt-bench/results/iphone/`.

| Engine | Set | WER % | Terms % | Median ms | p90 ms | First load | Warm load | Peak MB |
|---|---|---|---|---|---|---|---|---|
| Parakeet TDT 0.6b v3 (FluidAudio) | clean | 7.5 | 92 | 48 | 55 | 29 s | 0.1 s | 526 |
| Parakeet TDT 0.6b v3 (FluidAudio) | noisy | 15.5 | 75 | 53 | 57 | | | |
| Parakeet TDT 0.6b v2 (FluidAudio) | clean | 8.9 | 87 | 50 | 58 | 28 s | 0.2 s | 536 |
| Parakeet TDT 0.6b v2 (FluidAudio) | noisy | 17.4 | 68 | 54 | 58 | | | |
| Whisper small.en 217MB (WhisperKit) | clean | 9.9 | 83 | 357 | 396 | 32 s | 1.6 s | 494 |
| Whisper small.en 217MB (WhisperKit) | noisy | 19.7 | 66 | 352 | 397 | | | |
| Whisper large-v3 turbo 632MB (WhisperKit) | clean | 7.0 | 89 | 540 | 572 | 1125 s | 2.8 s | 789 |
| Whisper large-v3 turbo 632MB (WhisperKit) | noisy | 14.6 | 72 | 559 | 591 | | | |

The iPhone's peak footprint includes the memory the Neural Engine holds, which the Mac's number does not.

- **Accuracy matches the Mac.** The transcripts are the same on both devices, so WER and term recall carry across.
- **WhisperKit large-v3 turbo is the most accurate in WER** (7.0% clean, 14.6% noisy) but returns text 540 ms after speech, against Parakeet v3's 48 ms. That is the gap between a reply that feels immediate and one with a pause.
- **Its first load took 1125 s (almost 19 minutes)** while Core ML compiled it for the Neural Engine, and it needs 263 MB more than Parakeet v3. Voice mode would have to compile it in the background long before anyone taps the button.
- **Parakeet v3 recognises developer terms best (92%)**, loads in 29 s the first time, and adds FluidAudio's Silero VAD and Parakeet EOU for turn-taking (`wheel-e1xk.13`).

To re-run: build and install with `bash docs/voice-mode/stt-bench/build-ios.sh` (render the clips with `fixture/make.sh` first), keep the phone unlocked with STT Bench in the foreground, run `bash docs/voice-mode/stt-bench/run-iphone.sh`, and score with `python3 docs/voice-mode/stt-bench/score.py docs/voice-mode/stt-bench/fixture/utterances.tsv docs/voice-mode/stt-bench/results/iphone/*.pass2.json`. iOS suspends the app when it leaves the foreground, and the run stalls until it is back.

FluidAudio's `parakeet-redux` (about 220 MB, iOS 18+) is the smaller Parakeet if v3's 469 MB download matters on the phone. It is not measured here.

**Not measured:**
- Battery drain: the phone was on a cable, so a 10-minute voice session needs a wireless run.
- Real voices: every engine scores worse on real speech than on these synthetic system voices.

whisper.cpp on the iPhone: `build-xcframework.sh` builds every slice, but its last step, `xcodebuild -create-xcframework`, exits with `error: invalid argument '-jobs'`. Turnstile appends `-jobs 4` to that call, both through its shim and through `turnstile run`, although `turnstile classify` reports the command as pass through. On the Mac it is slower than WhisperKit at the same accuracy (793 ms for large-v3 turbo q5_0), so the iPhone gap does not change the pick.

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
