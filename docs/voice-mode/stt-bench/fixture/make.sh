#!/bin/bash
# Renders each utterance with a rotating system voice to 16 kHz mono WAV, plus a copy with pink noise at about 10 dB SNR.
set -euo pipefail
D=$(cd "$(dirname "$0")" && pwd)
mkdir -p "$D/clean" "$D/noisy"
voices=("Samantha" "Daniel" "Karen" "Moira" "Rishi" "Flo (English (US))")
i=0
tail -n +2 "$D/utterances.tsv" | while IFS=$'\t' read -r id text terms; do
  v=${voices[$((i % ${#voices[@]}))]}
  say -v "$v" -o "$D/clean/$id.aiff" "$text"
  ffmpeg -loglevel error -y -i "$D/clean/$id.aiff" -ar 16000 -ac 1 -af "apad=pad_dur=0.6,adelay=300" -c:a pcm_s16le "$D/clean/$id.wav"
  rm "$D/clean/$id.aiff"
  dur=$(sox --i -D "$D/clean/$id.wav")
  sox -n -r 16000 -c 1 -b 16 "$D/noise.wav" synth "$dur" pinknoise vol 0.24
  sox -m "$D/clean/$id.wav" "$D/noise.wav" "$D/noisy/$id.wav"
  echo "$id $v ${dur}s"
  i=$((i + 1))
done
rm -f "$D/noise.wav"
