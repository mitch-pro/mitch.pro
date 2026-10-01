#!/usr/bin/env python3
"""Add an original instrumental to a screen recording and encode a web tour.

Requires Python/numpy and ffmpeg/ffprobe. The soundtrack is synthesized here;
no sampled recordings or third-party music are used. The input is untouched.
Usage: python tools/create-site-tour.py /path/to/recording.mp4
"""
import json
import math
import pathlib
import subprocess
import sys
import tempfile
import wave

import numpy as np

ROOT = pathlib.Path(__file__).resolve().parents[1]
SOURCE = pathlib.Path(sys.argv[1])
OUT = ROOT / "webserver/media"
OUT.mkdir(parents=True, exist_ok=True)
duration = float(json.loads(subprocess.check_output([
    "ffprobe", "-v", "error", "-show_entries", "format=duration", "-of", "json", str(SOURCE)
]))["format"]["duration"])
sr = 44100
n = math.ceil(duration * sr)
mix = np.zeros((n, 2), dtype=np.float32)
rng = np.random.default_rng(20260930)
beat = 60 / 88
bar = beat * 4
# D minor 9 / B-flat major 7 / F major 9 / C suspended: 8-bar cycle.
chords = [(50, 57, 60, 64, 69), (46, 53, 57, 60, 65),
          (41, 53, 57, 60, 67), (48, 55, 58, 62, 67)]


def freq(midi):
    return 440 * 2 ** ((midi - 69) / 12)


def add(start, sound, gain, pan=0):
    offset = round(start * sr)
    end = min(n, offset + len(sound))
    if end <= offset:
        return
    sound = sound[:end - offset] * gain
    mix[offset:end, 0] += sound * math.sqrt((1 - pan) / 2)
    mix[offset:end, 1] += sound * math.sqrt((1 + pan) / 2)


def tone(midi, length, kind="key"):
    t = np.arange(round(length * sr), dtype=np.float32) / sr
    f = freq(midi)
    if kind == "pad":
        sound = (np.sin(2 * np.pi * f * t) + .25 * np.sin(2 * np.pi * f * 2.001 * t)
                 + .12 * np.sin(2 * np.pi * f * .998 * t))
        env = np.minimum(t / .8, 1) * np.minimum((length - t) / 1.2, 1)
    elif kind == "bass":
        sound = np.sin(2 * np.pi * f * t) + .13 * np.sin(2 * np.pi * f * 2 * t)
        env = np.minimum(t / .025, 1) * np.exp(-t * 2.3) * np.minimum((length - t) / .08, 1)
    else:
        sound = np.sin(2 * np.pi * f * t) + .3 * np.sin(2 * np.pi * f * 2 * t)
        env = np.minimum(t / .012, 1) * np.exp(-t * 3) * np.minimum((length - t) / .15, 1)
    return sound * env


for section in range(math.ceil(duration / (bar * 2))):
    start = section * bar * 2
    chord = chords[section % len(chords)]
    for i, note in enumerate(chord[1:]):
        add(start, tone(note, bar * 2 + 1, "pad"), .048, (i - 1.5) / 2)
    for step in range(8):
        add(start + step * beat, tone(chord[0] - 12, beat * .85, "bass"), .14)
    for step in range(16):
        note = chord[1 + [0, 2, 1, 3, 2, 1, 3, 1][step % 8]] + 12
        when = start + step * beat / 2
        sound = tone(note, 1.6)
        add(when, sound, .065, math.sin(step * 1.3) * .55)
        add(when + beat * .75, sound, .014, -math.sin(step * 1.3) * .55)
    # Ease the drums in after the opening phrase.
    if section < 2:
        continue
    for step in range(8):
        when = start + step * beat
        t = np.arange(round(.35 * sr), dtype=np.float32) / sr
        if step % 4 in (0, 2):
            kick = np.sin(2 * np.pi * (48 * t + 5 * (1 - np.exp(-t * 35)))) * np.exp(-t * 15)
            add(when, kick, .2)
        if step % 4 in (1, 3):
            noise = rng.normal(0, 1, len(t)).astype(np.float32)
            snare = (noise - np.roll(noise, 1)) * np.exp(-t * 30)
            add(when, snare, .023, -.12)
        for half in (0, .5):
            ht = np.arange(round(.09 * sr), dtype=np.float32) / sr
            noise = rng.normal(0, 1, len(ht)).astype(np.float32)
            hat = (noise - np.roll(noise, 1)) * np.exp(-ht * 65)
            add(when + half * beat, hat, .011 if half == 0 else .007, .3)

fade = np.minimum(np.arange(n) / (sr * 3), 1) * np.minimum((n - np.arange(n)) / (sr * 5), 1)
mix *= fade[:, None]
mix = np.tanh(mix * 1.3)
mix *= .65 / max(float(np.max(np.abs(mix))), .01)
with tempfile.TemporaryDirectory(prefix="mitch-tour-") as tmp:
    wav = pathlib.Path(tmp) / "browser-days.wav"
    with wave.open(str(wav), "wb") as w:
        w.setnchannels(2)
        w.setsampwidth(2)
        w.setframerate(sr)
        w.writeframes((mix * 32767).astype("<i2").tobytes())
    subprocess.run([
        "ffmpeg", "-hide_banner", "-y", "-i", str(SOURCE), "-i", str(wav),
        "-map", "0:v:0", "-map", "1:a:0", "-vf", "scale=1920:-2,format=yuv420p",
        "-c:v", "libx264", "-preset", "medium", "-crf", "27", "-threads", "4",
        "-c:a", "aac", "-b:a", "128k", "-movflags", "+faststart", "-shortest",
        "-metadata", "title=mitch.pro — a look around",
        "-metadata", "comment=Original instrumental: Browser Days. Synthesized by tools/create-site-tour.py.",
        str(OUT / "site-tour-v1.mp4")
    ], check=True)
    subprocess.run([
        "ffmpeg", "-hide_banner", "-loglevel", "error", "-y", "-ss", "52", "-i", str(SOURCE),
        "-frames:v", "1", "-vf", "scale=1440:-2", "-quality", "85", str(OUT / "site-tour-poster-v1.webp")
    ], check=True)
print(f"Created {OUT / 'site-tour-v1.mp4'} ({duration:.2f}s) with original music.")
