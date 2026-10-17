#!/usr/bin/env python3
"""Encode a versioned site tour, optionally retimed to a supplied soundtrack.

Requires ffmpeg and ffprobe. Source recordings and audio are left untouched.
"""
import argparse
import json
from pathlib import Path
import subprocess


def probe(path):
    return json.loads(subprocess.check_output([
        "ffprobe", "-v", "error", "-show_entries",
        "format=duration:stream=codec_type", "-of", "json", str(path),
    ]))


def timing_filter(chapters, source_duration, duration):
    points = [(float(c["source"]), float(c["time"])) for c in chapters]
    if not points or points[0] != (0, 0):
        raise ValueError("The timing map must start at zero.")
    points.append((source_duration, duration))
    expressions = []
    for (start, output), (end, next_output) in zip(points, points[1:]):
        if end <= start or next_output <= output:
            raise ValueError("Chapter times must increase.")
        if next_output - output > end - start:
            raise ValueError("The timing map must only speed up the recording.")
        ratio = (next_output - output) / (end - start)
        expressions.append((end, f"({output:.9f}+(T-{start:.9f})*{ratio:.12f})"))
    expression = expressions[-1][1]
    for end, current in reversed(expressions[:-1]):
        expression = f"if(lt(T,{end:.9f}),{current},{expression})"
    return f"setpts=PTS-STARTPTS,setpts='({expression})/TB'"


def soundtrack_filter(soundtrack, duration):
    # Lift quiet passages smoothly, keeping stereo channels linked. Measure the
    # processed track before normalizing so the final mix reaches -14 LUFS.
    preparation = (
        f"atrim=duration={duration},asetpts=PTS-STARTPTS,"
        "dynaudnorm=f=500:g=31:p=0.8:m=10:r=0.18,"
        f"afade=t=out:st={duration-1}:d=1"
    )
    target = "loudnorm=I=-14:TP=-2:LRA=11"
    measurement = subprocess.run([
        "ffmpeg", "-hide_banner", "-nostats", "-i", str(soundtrack), "-vn",
        "-af", f"{preparation},{target}:print_format=json", "-f", "null", "-",
    ], check=True, capture_output=True, text=True)
    stats, _ = json.JSONDecoder().raw_decode(measurement.stderr[measurement.stderr.rfind("{"):])
    normalization = (
        f"{target}:measured_I={stats['input_i']}:measured_TP={stats['input_tp']}:"
        f"measured_LRA={stats['input_lra']}:measured_thresh={stats['input_thresh']}:"
        f"offset={stats['target_offset']}"
    )
    return f"{preparation},{normalization}"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", type=Path)
    parser.add_argument("--version", type=int, required=True)
    parser.add_argument("--poster-time", type=float, default=5)
    parser.add_argument("--soundtrack", type=Path)
    parser.add_argument("--timing", type=Path)
    parser.add_argument("--copy-video", action="store_true",
                        help="Replace audio in an existing edit without re-encoding video or creating a poster.")
    args = parser.parse_args()
    if args.copy_video and (not args.soundtrack or args.timing):
        parser.error("--copy-video requires --soundtrack and cannot be used with --timing.")
    for path in [args.source, args.soundtrack, args.timing]:
        if path is not None and not path.is_file():
            parser.error(f"Input does not exist: {path}")
    if args.version < 1:
        parser.error("The asset version must be positive.")
    source = probe(args.source)
    source_duration = float(source["format"]["duration"])
    if not 0 <= args.poster_time < source_duration:
        parser.error("The poster time must fall within the source recording.")
    timing = json.loads(args.timing.read_text()) if args.timing else None
    duration = float(timing["duration"]) if timing else source_duration
    if not 0 < duration <= source_duration:
        parser.error("The output duration must be positive and no longer than the source.")
    if args.soundtrack and float(probe(args.soundtrack)["format"]["duration"]) < duration:
        parser.error("The soundtrack is shorter than the output.")

    out = Path(__file__).resolve().parents[1] / "webserver/media"
    out.mkdir(parents=True, exist_ok=True)
    video = out / f"site-tour-v{args.version}.mp4"
    poster = out / f"site-tour-poster-v{args.version}.webp"
    if video.exists() or (not args.copy_video and poster.exists()):
        parser.error("These assets already exist. Use a new version number.")

    vf = timing_filter(timing["chapters"], source_duration, duration) if timing else "setpts=PTS-STARTPTS"
    vf += ",fps=30,scale=min(1920\\,iw):-2:flags=lanczos,format=yuv420p"
    if timing:
        vf += f",fade=t=in:st=0:d=0.3,fade=t=out:st={duration-1.5}:d=1.5"
    command = ["ffmpeg", "-hide_banner", "-loglevel", "warning", "-n",
               "-threads", "4", "-i", str(args.source)]
    if args.soundtrack:
        command += ["-i", str(args.soundtrack), "-map", "0:v:0", "-map", "1:a:0",
                    "-af", soundtrack_filter(args.soundtrack, duration)]
    else:
        command += ["-map", "0:v:0", "-map", "0:a?"]
    if args.copy_video:
        command += ["-c:v", "copy"]
    else:
        command += ["-vf", vf, "-c:v", "libx264", "-preset", "medium",
                    "-crf", "22", "-threads", "4"]
    if args.soundtrack or any(s["codec_type"] == "audio" for s in source["streams"]):
        command += ["-c:a", "aac", "-b:a", "192k", "-ar", "48000"]
    command += ["-t", str(duration), "-movflags", "+faststart", "-map_metadata", "-1",
                "-metadata", "title=mitch.pro site tour", str(video)]
    subprocess.run(command, check=True)
    if args.copy_video:
        print(f"Created {video.name} ({duration:.3f}s), preserving the original video stream.")
        return
    subprocess.run([
        "ffmpeg", "-hide_banner", "-loglevel", "error", "-n",
        "-ss", str(args.poster_time), "-threads", "4", "-i", str(args.source),
        "-frames:v", "1", "-vf", "scale=min(1920\\,iw):-2", "-quality", "88",
        str(poster),
    ], check=True)
    print(f"Created {video.name} ({duration:.3f}s) and {poster.name}.")


if __name__ == "__main__":
    main()
