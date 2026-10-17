#!/usr/bin/env python3
"""Encode a screen recording and extract its poster without adding audio.

Requires ffmpeg and ffprobe. The source recording is left untouched.
Usage: python tools/create-site-tour.py recording.mp4 --version 2 --poster-time 5
"""
import argparse
import json
from pathlib import Path
import subprocess


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", type=Path)
    parser.add_argument("--version", type=int, default=2)
    parser.add_argument("--poster-time", type=float, default=5)
    args = parser.parse_args()
    if not args.source.is_file():
        parser.error("The source recording does not exist.")
    if args.version < 1:
        parser.error("The asset version must be positive.")

    metadata = json.loads(subprocess.check_output([
        "ffprobe", "-v", "error", "-show_entries", "format=duration",
        "-of", "json", str(args.source),
    ]))
    duration = float(metadata["format"]["duration"])
    if not 0 <= args.poster_time < duration:
        parser.error("The poster time must fall within the recording.")

    out = Path(__file__).resolve().parents[1] / "webserver/media"
    out.mkdir(parents=True, exist_ok=True)
    video = out / f"site-tour-v{args.version}.mp4"
    poster = out / f"site-tour-poster-v{args.version}.webp"
    if video.exists() or poster.exists():
        parser.error("These assets already exist. Use a new version number.")

    subprocess.run([
        "ffmpeg", "-hide_banner", "-loglevel", "warning", "-n",
        "-threads", "4", "-i", str(args.source),
        "-map", "0:v:0", "-map", "0:a?",
        "-vf", "fps=30,scale=min(1920\\,iw):-2:flags=lanczos,format=yuv420p",
        "-c:v", "libx264", "-preset", "medium", "-crf", "23", "-threads", "4",
        "-c:a", "aac", "-b:a", "128k", "-movflags", "+faststart",
        "-map_metadata", "-1", "-metadata", "title=mitch.pro site tour",
        str(video),
    ], check=True)
    subprocess.run([
        "ffmpeg", "-hide_banner", "-loglevel", "error", "-n",
        "-ss", str(args.poster_time), "-threads", "4", "-i", str(args.source),
        "-frames:v", "1", "-vf", "scale=min(1440\\,iw):-2", "-quality", "85",
        str(poster),
    ], check=True)
    print(f"Created {video.name} ({duration:.2f}s) and {poster.name}.")


if __name__ == "__main__":
    main()
