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

例の ID は実際の応答に合わせてください。`screenshot` は画像 ID、PNG のパス、寸法、現在のフォーカスを返します。直前の画像 ID を指定して同じ領域を撮ると、変化がなければ `unchanged: true` が返ります。クリック座標は指定した画像の左上が原点です。キーコードは Linux の evdev 番号です。PNG と `actions.jsonl` はセッション中に一時ディレクトリから確認できます。残す場合は停止前にコピーしてください。

## 検証状況

GTK の Wayland ウィンドウ、モーダル、XWayland 上の xterm で撮影と入力を確認済みです。Pixman と Intel GPU の EGL/GLES では、上下で色が異なるウィンドウとその一部を撮影して画素を確認しました。検証機上の NVIDIA GPU と nanoem は実機検証が必要です。
