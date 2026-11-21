"""Headless compositor integration checks using a GTK fixture and xterm."""

import argparse
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time

from PIL import Image

HERE = Path(__file__).resolve().parent


def wait_for(check):
    deadline = time.monotonic() + 8
    while time.monotonic() < deadline:
        result = check()
        if result:
            return result
        time.sleep(0.05)
    raise AssertionError("timed out waiting for the compositor or client")


def run_session(binary, renderer, apps, check):
    with tempfile.TemporaryDirectory(prefix="rscu-smoke-") as directory:
        directory = Path(directory)
        command = [str(binary), "--socket", str(directory / "control")]
        env = {**os.environ, "GSK_RENDERER": "cairo"}
        with (directory / "server.log").open("w+") as log:
            process = subprocess.Popen(
                command + ["run", "--renderer", renderer] + apps(directory),
                env=env,
                stdout=log,
                stderr=log,
            )

            def ctl(*args, error=None):
                result = subprocess.run(
                    command + [str(arg) for arg in args],
                    capture_output=True,
                    text=True,
                    timeout=4,
                )
                if error:
                    assert result.returncode != 0 and error in result.stderr, result
                    return None
                assert result.returncode == 0, result
                return json.loads(result.stdout)

            try:
                wait_for(lambda: (directory / "control").exists() or process.poll() is not None)
                assert process.poll() is None
                check(ctl, directory)
            except BaseException:
                log.seek(0)
                print(log.read(), file=sys.stderr)
                raise
            finally:
                if process.poll() is None:
                    try:
                        ctl("stop")
                        assert not (directory / "control").exists()
                        process.wait(timeout=5)
                    except (AssertionError, subprocess.SubprocessError, ValueError):
                        process.kill()
                        process.wait()


def check_pixels(ctl, _directory):
    window = wait_for(lambda: ctl("windows"))[0]["id"]
    captured = ctl("screenshot", window)

    with Image.open(captured["path"]) as image:
        assert image.getpixel((40, 20)) == (255, 0, 0)
        assert image.getpixel((40, 180)) == (0, 0, 255)

    crop = ctl("screenshot", window, "--crop", 38, 18, 5, 5)
    with Image.open(crop["path"]) as image:
        assert image.size == (5, 5)
        assert image.getpixel((2, 2)) == (255, 0, 0)


def input_apps(directory, backend):
    return [
        backend, "env", f"GDK_BACKEND={backend[2:]}",
        sys.executable, str(HERE / "client.py"), str(directory / "modal-input"),
        "--x11", "xterm", "-T", "rscu-xterm", "-e", "sh", "-c",
        'stty -echo; IFS= read -r line; printf "%s\\n" "$line" > "$1"; sleep 10',
        "sh", str(directory / "xterm-input"),
    ]


def check_input(ctl, directory):
    def lookup(title):
        return next((w for w in ctl("windows") if w["title"] == title), None)

    main = wait_for(lambda: lookup("rscu-main"))["id"]
    other = wait_for(lambda: lookup("rscu-other"))["id"]
    xterm = wait_for(lambda: lookup("rscu-xterm"))["id"]
    assert ctl("activate", main)["focus"] == main
    image = ctl("screenshot", main)["image"]
    assert wait_for(lambda: ctl("screenshot", main, image).get("unchanged"))

    ctl("activate", other)
    ctl("click", image, 50, 50, error="stale image")
    original = ctl("screenshot", other)
    ctl("key", other, 50)
    wait_for(lambda: ctl("screenshot", other)["width"] == 1280)
    ctl("key", other, 22)
    wait_for(lambda: ctl("screenshot", other)["width"] == original["width"])
    ctl("maximize", other)
    wait_for(lambda: ctl("screenshot", other)["width"] == 1280)
    ctl("restore", other)
    wait_for(lambda: ctl("screenshot", other)["width"] == original["width"])
    menu = ctl("screenshot", other)
    ctl("click", menu["image"], menu["width"] // 2, menu["height"] // 2)
    wait_for(lambda: ctl("screenshot", other)["height"] > menu["height"])
    ctl("key", other, 1)
    wait_for(lambda: ctl("screenshot", other)["height"] == menu["height"])
    ctl("activate", main)
    crop = ctl("screenshot", main, "--crop", 200, 160, 100, 100)
    ctl("click", crop["image"], 50, 50)

    dialog = wait_for(lambda: lookup("rscu-modal"))
    assert dialog["parent"] == main and dialog["modal"]
    modal = dialog["id"]
    assert ctl("activate", main)["focus"] == modal
    ctl("key", main, 30, error="does not have focus")
    ctl("screenshot", modal)
    ctl("key", modal, 30)
    wait_for(lambda: (directory / "modal-input").exists())
    assert (directory / "modal-input").read_text() == "modal received a\n"
    wait_for(lambda: lookup("rscu-main")["focused"])

    ctl("activate", xterm)
    ctl("screenshot", xterm)
    ctl("key", xterm, 30, 42)
    ctl("key", xterm, 28)
    wait_for(lambda: (directory / "xterm-input").exists())
    assert (directory / "xterm-input").read_text() == "A\n"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=Path("target/debug/rscu"))
    parser.add_argument("--renderer", choices=["pixman", "gles"], default="pixman")
    args = parser.parse_args()
    run_session(
        args.binary.resolve(), args.renderer,
        lambda _: ["--wayland", sys.executable, str(HERE / "pixels.py")], check_pixels,
    )
    for backend in ["--wayland", "--x11"]:
        run_session(
            args.binary.resolve(), args.renderer,
            lambda directory: input_apps(directory, backend), check_input,
        )
        print(f"Passed {args.renderer} pixel and {backend[2:]} input checks")


if __name__ == "__main__":
    main()
