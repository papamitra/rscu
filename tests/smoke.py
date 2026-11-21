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


def mouse_apps(directory, backend):
    apps = [
        backend, "env", f"GDK_BACKEND={backend[2:]}",
        sys.executable, str(HERE / "mouse_client.py"), str(directory / "mouse-events"),
    ]
    if backend == "--x11":
        apps += [
            "--x11", "sh", "-c", 'exec stdbuf -oL xev -name rscu-wheel -event button > "$1"',
            "sh", str(directory / "wheel-events"),
        ]
    return apps


def lookup_window(ctl, title):
    return wait_for(lambda: next((w["id"] for w in ctl("windows") if w["title"] == title), None))


def check_mouse(ctl, directory, backend):
    window = lookup_window(ctl, "rscu-mouse")
    ctl("activate", window)
    image = ctl("screenshot", window)["image"]
    log = directory / "mouse-events"

    def events():
        return [line.split() for line in log.read_text().splitlines()] if log.exists() else []

    ctl("move", image, 80, 85)
    wait_for(lambda: ["motion", "80", "85"] in events())
    assert not any(kind == "press" for kind, *_ in events())

    ctl("click", image, 65, 65, "right")
    ctl("click", image, 90, 65, "middle")
    ctl("double-click", image, 135, 85)
    for button, count in [("3", "1"), ("2", "1"), ("1", "2")]:
        wait_for(lambda: ["press", button, count] in events())

    if backend == "--wayland":
        ctl("scroll", image, 120, 100, 2, -3)
        wait_for(lambda: any(kind == "scroll" and float(x) > 0 for kind, x, _ in events()))
        wait_for(lambda: any(kind == "scroll" and float(y) < 0 for kind, _, y in events()))

    ctl("drag", image, 50, 65, 190, 145)
    wait_for(lambda: ["drag", "140", "80"] in events())
    assert any(kind == "motion" and 50 < int(x) < 190 and 65 < int(y) < 145
               for kind, x, y in events())
    ctl("drag", image, 50, 65, 500, 145, error="outside the image")
    ctl("scroll", image, 120, 100, 0, 0, error="nonzero wheel steps")
    assert ["drag", "140", "80"] in events()

    if backend == "--x11":
        # Check legacy X11 wheel buttons directly, including their step counts.
        wheel = lookup_window(ctl, "rscu-wheel")
        ctl("activate", wheel)
        image = ctl("screenshot", wheel)["image"]
        ctl("scroll", image, 55, 50, 2, -3)
        log = directory / "wheel-events"
        wait_for(lambda: log.read_text().count("button 4,") >= 6)
        output = log.read_text()
        assert output.count("button 7,") == 4 and output.count("button 4,") == 6, output


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
        run_session(
            args.binary.resolve(), args.renderer,
            lambda directory: mouse_apps(directory, backend),
            lambda ctl, directory: check_mouse(ctl, directory, backend),
        )
        print(f"Passed {args.renderer} pixel and {backend[2:]} keyboard and mouse checks")


if __name__ == "__main__":
    main()
