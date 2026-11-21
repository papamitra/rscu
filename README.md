# rscu

Rust と Smithay で動く Linux Computer Use 用の専用 Wayland コンポジタです。操作するウィンドウを ID で選び、そのウィンドウの撮影と入力をローカル CLI から行います。構成と検証項目は [設計書](COMPUTER_USE_DESIGN.md) にあります。

## 起動

ビルドには Rust 1.94（検証済み）と Wayland、libxkbcommon、Pixman の開発パッケージが必要です。X11 アプリを使う場合は `Xwayland`、GLES を使う場合は EGL ライブラリと対応ドライバーも必要です。

```sh
cargo build --target-dir target
dir=$(mktemp -d)
target/debug/rscu --socket "$dir/control" run \
  --wayland weston-simple-shm --x11 xterm &
```

`--wayland APP [ARGS...]` と `--x11 APP [ARGS...]` はそれぞれ繰り返せます。既定の描画は Pixman です。GPU 描画を試す場合は `run` の直後に `--renderer gles` を付けます。複数の GPU がある環境では `RSCU_RENDER_NODE=/dev/dri/renderD128` のように指定できます。

## 操作

```sh
ctl() { target/debug/rscu --socket "$dir/control" "$@"; }
ctl windows
ctl activate 1
ctl screenshot 1
ctl screenshot 1 --crop 10 20 100 80
ctl screenshot 1 1
ctl click 1 30 40
ctl key 1 30
ctl stop
rmdir "$dir"
```

例の ID は実際の応答に合わせてください。`screenshot` は画像 ID、PNG のパス、寸法、現在のフォーカスを返します。直前の画像 ID を指定して同じ領域を撮ると、変化がなければ `unchanged: true` が返ります。クリック座標は指定した画像の左上が原点です。キーコードは Linux の evdev 番号です。修飾キーは末尾に追加できます。例として `ctl key 1 24 29` は Ctrl+O を送ります。PNG と `actions.jsonl` はセッション中に一時ディレクトリから確認できます。残す場合は停止前にコピーしてください。

## 検証

GTK 4 の Python バインディング、Pillow、xterm、Xwayland がある環境では、次の操作テストを実行できます。

```sh
python3 tests/smoke.py
python3 tests/smoke.py --renderer gles
```

Pixman と GLES で、PNG の画素と部分撮影、複数ウィンドウの切り替え、最大化と復元、Wayland と X11 のモーダル、キー入力を検証しました。

検証機の Ubuntu 22.04 と XWayland 22.1.1 では、NVIDIA RTX 6000 Ada での GLES 合成と、専用 XWayland での NVIDIA GLX を確認しました。nanoem の起動、PNG 撮影、最大化と復元、指定座標へのクリック、対象ウィンドウへのキーイベントまで確認しました。検証前後で通常デスクトップのアクティブウィンドウ ID は同じでした。
