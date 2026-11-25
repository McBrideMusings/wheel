# Voice mode

Design from the 22 Nov 2026 session, written to be built on its own. It sits alongside the orchestrator redesign (`docs/orchestrator-redesign/index.md`, §8.9) and follows that doc's layout rules: one header row, the composer only on agent panes, badges, and the settings gear. Built so far: composer dictation (§5.8) and voice mode (§5.9 for the Mac; on the iPhone, WheelUI's `Voice` and `VoiceSession` behind a waveform button beside the mic, which Settings › Speech › Composer can hide, as it can the mic).

## 1. What it is

A full-screen, back-and-forth spoken conversation with **any agent pane**: the orchestrator, an agent in a space, or an agent opened from Agents mode. There is no in-chat voice bar. The voice button in an agent's composer opens voice mode, and the X closes it.

The orchestrator is the main user: you talk through what's in the queue, give instructions and approve things while away from the keyboard.

## 2. Decisions already made

- **Transcription runs on the device**, on both the phone and the Mac. Text goes over the wire to wheeld, never audio.
- **Not Apple's built-in speech recognition** (`SFSpeechRecognizer`). Use a model that's downloaded after install and cached. Check existing libraries before writing one; WhisperKit (Whisper on Core ML) and whisper.cpp are the obvious candidates.
- **Replies use a system voice.** No voice cloning.
- **One voice client at a time** across the phone and the Mac. Starting voice on one device ends it on the other.
- **iOS keeps running in the background** through an active audio session, with the screen locked (§5.6).
- **Voice is a part any agent view can add**, not something built only for the orchestrator. If it's cheaper, it can run in the client natively.
- **Approvals are inferred from speech.** The agent says what it approved, and only irreversible actions need a spoken yes.

## 3. The conversation model

- **It's the same chat.** What you say is sent as your message; every reply is written into the chat as well as spoken. Voice and text mix freely, and leaving voice mode leaves the full transcript behind.
- **In the orchestrator, topics apply unchanged** (orchestrator doc §4.2): "next", "back to X" and "triage" work by voice, and nothing from the queue is spoken unless you ask.
- **Turn-taking:** detection on the device decides when you've stopped speaking (a voice-activity detector, e.g. Silero VAD), then sends the transcript. Hold-to-talk is a setting for noisy places.
- **Interrupting:** tap the orb, or talk while the agent speaks. A tap stops speech at once. On the iPhone a sound over the reply first lowers it to a fifth of its volume; when the sound ends it is transcribed, and only words stop the reply and go to the agent as a turn. Hums, clicks and nods ("yeah", "okay") let the reply carry on at full volume and are dropped; a hum or a click heard while listening is not sent either (WheelUI's `SpokenFiller`). A stopped reply's unspoken rest is dropped from audio (it's still in the chat).
- **Spoken replies stay short.** While the voice client holds the `voice` lease and names an agent (`agent_voice`), each of that agent's turns carries the voice line (`daemon/src/voice.md`) as hidden context: short spoken sentences, summarize rather than read out code, and put anything long on screen. Claude takes it through a `UserPromptSubmit` hook, pi through Wheel's extension adding it to each run's system prompt; Codex refuses (ADR-0055).
- **Showing things:** when the agent refers to a diff, pane or doc, it shows it above the orb with `wheel agent card`. Tapping it opens it full screen.

## 4. Approvals

| Request | What happens |
|---|---|
| Reversible, and what you said covers it | Approved. A one-line green report on screen, and spoken in one short sentence |
| Reversible, not covered | Spoken question: "wheel-2 wants to run npm test. OK?" |
| Cannot be undone (push, delete, deploy, server restart) | An orange card on screen plus a spoken question. It waits for a clear yes or no. Tapping Allow or Deny on the card also works |

What counts as irreversible reuses wheel's existing check (`wheel_proto::is_irreversible`). Which rule applies stays configurable per the orchestrator doc: surface every request, let the agent decide, or use hooks to force it to ask.

## 5. Screens

### 5.1 Listening
![](images/vo2-full.png)
The header shows the agent and its context line: the topic for the orchestrator, space and harness for others. On the right are the panes icon (with a badge) and the queue (orchestrator only). In the middle, the orb and its state. At the bottom: + (attach), "Type instead…", mute and end.

### 5.2 Speaking
![](images/vo3-speak.png)
A live caption of the spoken reply, with "Speaking · tap to interrupt".

### 5.3 Approval by voice
![](images/vo4-approve.png)
A reversible approval reported in one line; an irreversible request as a card plus a spoken question.

### 5.4 Showing a pane
![](images/vo5-show.png)
What the agent refers to appears above the orb, which shrinks to make room.

### 5.5 Panes during voice
![](images/vo7-panes.png)
Swipe left, or tap the panes icon: the panes come back full screen and voice shrinks to a floating pill (orb, caption, mute, end). Voice keeps listening and speaking. Tap the orb to return to full screen. Tapping or swiping on a pane hides the keyboard, as everywhere else.

### 5.6 Locked
![](images/vo6-lock.png)
A Live Activity on the lock screen and Dynamic Island shows the state (listening, speaking, thinking), the agent and its topic, with mute and end.

### 5.7 Any agent
![](images/vo8-agent.png)
A Spaces agent pane (W11482 in llama-broker) in voice mode. The layout is the same without the queue.

### Orb states
| State | Look |
|---|---|
| Listening | Blue, with rings that pulse with your volume |
| Thinking | Gray, slowly breathing |
| Speaking | Bright, moving with the output level |
| Muted | Dimmed; the mute button is lit |

## 5.8 Composer dictation (built)

The microphone in an agent's composer, on the Mac (GPUI, in the row under the draft, left of the round button) and the iPhone (left of send). Press it, speak, press again: the recording becomes text added to the draft of the window that pressed, and the person sends it as typed text with `agent_send`. Parakeet TDT 0.6b runs on the device, v3 (multilingual) unless the person picks v2 (English only) in Settings › Speech: on the iPhone through FluidAudio (WheelUI's `Dictation`, `docs/voice-mode/stt-engines.md`), on the Mac through sherpa-onnx in the GPUI client's own Rust (§5.9). The first press with no model on the device shows what the download fetches with Download and Cancel: for v3 on the iPhone 483.1 MB from `huggingface.co/FluidInference/parakeet-tdt-0.6b-v3-coreml` into FluidAudio's cache, on the Mac 670.6 MB from `huggingface.co/csukuangfj/sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8` into `~/Library/Application Support/Wheel/Speech/parakeet`; for v2 464.4 MB from `huggingface.co/FluidInference/parakeet-tdt-0.6b-v2-coreml` on the iPhone and 661.3 MB from `huggingface.co/csukuangfj/sherpa-onnx-nemo-parakeet-tdt-0.6b-v2-int8` into `…/Speech/parakeet-v2` on the Mac.

Settings › Speech, on the Mac and the iPhone, lists each model (both Parakeets and Silero VAD) with its size, source, folder and whether it is on the device, with Download or Redownload and Remove, and the picker between v3 and v2. A model dictation or voice mode is using cannot be removed, replaced or swapped while it runs; picking a model that is not on the device offers its download first, and it becomes the choice once the download lands. Only one download runs at a time: a second, from Settings or the composer, is refused and says so. The choice persists on each device: the Mac keeps it in the speech folder's `speech-to-text` file, the iPhone in `UserDefaults` (`speech.stt`). The Mac checks every download against its pinned SHA-256; FluidAudio on the iPhone publishes no hashes to pin. Nothing in it talks to wheeld, so it works with wheeld out of reach until the send.

## 6. Where things run

| Part | Where |
|---|---|
| Microphone, voice detection, transcription | On the device (phone or Mac) |
| Text to speech | On the device, system voice |
| The conversation | The agent's own session in wheeld; voice sends text with `agent_send` and reads the transcript stream like any client |
| Lock to one active voice client | wheeld (a small generic lock per agent, or for the whole daemon) |
| Voice line while active | wheeld answers the Claude agent's `UserPromptSubmit` hook with it while the `voice` lease holder names that agent (`agent_voice`); it clears with the lease (ADR-0055) |
| Panes during voice | Client: voice is a layer over the normal agent view, so pane navigation is unchanged |

## 7. Settings

These sit under Settings in the orchestrator doc:
- **Voice:** the transcription model (download size, keep on device), the system voice and speed, hold-to-talk vs automatic turn-taking, and whether to speak replies.
- **Approvals by voice:** off / reversible only / all with a spoken yes for irreversible.

## 5.9 Voice mode on the Mac (built)

The round button at the right end of the composer's lower row is voice mode while the draft is empty and the agent idle, Send while there is a draft or a file, and Stop while the agent works; with a wheeld that lacks `lease_claim` or `agent_voice` the empty state is a dimmed Send. Voice mode covers the agent pane whose button opened it; the rest of the grid stays as it was.

- **Audio, in Rust (ADR-0056):** `clients/gpui/src/speech/`, one app-wide GPUI entity every window observes, with its state and rules in `clients/gpui/core/src/dictation.rs` and `voice.rs`. The microphone and the speaker go through AVFoundation by objc2: one `AVAudioEngine` whose input node has voice processing on, an input tap converted by `AVAudioConverter` to 16 kHz mono, and the reply written by `AVSpeechSynthesizer` (`writeUtterance:toBufferCallback:`) and played by an `AVAudioPlayerNode` on the same engine, so the echo canceller hears what it plays and the person can talk over it. Dictation records through its own engine without voice processing. A worker thread holds the models through sherpa-onnx (`sherpa-rs`, linked statically): Silero VAD (0.6 MB from `github.com/k2-fsa/sherpa-onnx`; a turn ends after 0.6 s of silence, at most 30 s long) and Parakeet TDT 0.6b int8 (v3, or v2 when Settings › Speech picks it) on ONNX Runtime's CPU provider (two threads, which beat one and four on this Mac). Its Core ML provider runs this int8 model and gives the same text, but loaded in 8.3 to 9.3 s against 2.8 to 4.3 s and transcribed a 1.8 s clip in 0.41 to 1.58 s against 0.32 to 0.50 s, so the Mac stays on the CPU. FluidAudio, the iPhone's engine, is an iOS-only dependency of WheelUI and never reaches the Mac. A turn keeps the 0.4 s before the VAD heard speech start, which would otherwise lose the first word, and is padded to a second before Parakeet reads it. When the agent's turn ends, its reply (Markdown made speakable, `core/src/spoken.rs`) is spoken and captioned. Speaking over the reply, or pressing the orb, stops the speech; the chat keeps the whole reply. The orb's level changes at most 15 times a second.
- **Models (ADR-0013):** `~/Library/Application Support/Wheel/Speech/`, one folder per model (`parakeet`, `parakeet-v2`, `silero-vad`; `WHEEL_SPEECH_MODELS` moves it for a check). Before the first download the composer or the layer names each model, its size, its source and the folder, with Download and Cancel. A download fills a staging folder beside the model's, checks each file's pinned SHA-256 and renames the folder into place, so a model is whole or absent; Cancel stops it and leaves nothing.
- **Measured on this Mac** (Apple silicon, other builds running): in the app Parakeet loads in 2.8 to 3.6 s with two threads, a 1.8 s dictated clip transcribes in 270 to 420 ms, and a 1.6 s voice turn in 210 ms to 1.4 s. Standalone, two threads transcribed a 1.8 s clip in 0.4 to 1 s against 0.8 to 1.5 s with one, and an 8.3 s clip in 4.3 to 10.6 s against 6.8 to 8.4 s with one; four threads were slower again under load.
- **Microphone permission:** the app asks macOS the first time a press needs the microphone (`AVAudioApplication`); the Info.plist's `NSMicrophoneUsageDescription` says why. Refused, the composer or the layer says where to allow it.
- **Several windows:** the app has one recorder and one voice audio. Only the window that pressed the microphone takes its text; only the window holding the voice session sends what the person said; a stop names its window and agent, so one window's X never ends another's voice, and voice opened in another window ends this one's layer.
- **wheeld:** opening claims the `voice` lease, then names the agent with `agent_voice`; X or Esc clears both. Another client taking the lease closes voice mode here, and the composer says "Voice mode moved to another device."
- **Layer:** the pane's label and "space · harness" at the top with the panes button on the right (no badge: the Mac grid shows its panes); the orb and its state word in the middle; at the bottom +, "Type instead…", mute and X. The first start with a model missing names both downloads with a Download button.
- **Pill:** the panes button, "Type instead…" and a card's chip shrink the layer to a pill over the transcript's foot (orb, state, caption, mute, X), and pressing its orb brings the layer back. A card the agent shows during voice is a chip above the orb, which shrinks; pressing it shows the card in the transcript under the pill.
- **Ends with its pane:** closing the window, closing the pane, or showing another space in that window ends voice mode, since nothing on screen would say the microphone is open. A reply that arrives while the person is still speaking waits until their turn is sent.
- **Not yet:** hold-to-talk, the Voice settings of §7, and approvals by voice (§4).

## 8. Open items

1. Measure battery on the phone over a 10-minute voice session.
2. Wheeld lock for one active voice client.
3. Live Activity and audio session background on iOS.
4. The part shared by every agent view: the orb, captions, the pill, and the approval card.

Design canvas: https://claude.ai/artifact/XfjmQGaLfjHkYHJ2338FcF (Mobile page, Voice mode row).
