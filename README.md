# rscu

Rust と Smithay で動く Linux Computer Use 用の専用 Wayland コンポジタです。操作するウィンドウを ID で選び、そのウィンドウの撮影と入力をローカル CLI から行います。構成と検証項目は [設計書](COMPUTER_USE_DESIGN.md) にあります。

## 起動

ビルドには Rust 1.94（検証済み）と CMake、Wayland、libxkbcommon、Pixman の開発パッケージが必要です。X11 アプリを使う場合は `Xwayland`、GLES を使う場合は EGL ライブラリと対応ドライバーも必要です。

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
ctl maximize 1
ctl screenshot 1
ctl screenshot 1 --crop 10 20 100 80
ctl screenshot 1 1
ctl click 1 30 40
ctl key 1 30
ctl restore 1
ctl stop
rmdir "$dir"
```

例の ID は実際の応答に合わせてください。`screenshot` は画像 ID、PNG のパス、寸法、現在のフォーカスを返します。直前の画像 ID を指定して同じ領域を撮ると、変化がなければ `unchanged: true` が返ります。クリック座標は指定した画像の左上が原点です。キーコードは Linux の evdev 番号です。修飾キーは末尾に追加できます。例として `ctl key 1 24 29` は Ctrl+O を送ります。PNG と `actions.jsonl` はセッション中に一時ディレクトリから確認できます。残す場合は停止前にコピーしてください。

### マウス操作

```sh
ctl move 1 30 40
ctl click 1 30 40 right
ctl click 1 30 40 middle
ctl double-click 1 30 40
ctl scroll 1 30 40 0 -3
ctl drag 1 30 40 220 160
```

座標は画像 ID の PNG を基準にします。部分撮影の画像では切り出し領域の左上が原点です。スクロールの最後の２引数は横・縦のホイール段数で、正は右・下、負は左・上です。移動は１回の座標更新で、ドラッグは中間座標を送ります。ドラッグ中に対象が変わるとボタンを解放して中断します。

## MCP で操作する

`crates/rscu-mcp` は stdio の MCP サーバーです。`crates/rscu-control` の操作 API を使い、起動済みの rscu に Unix ソケットで接続します。初版は MCP サーバー1プロセスにつき、`--socket` で指定した rscu セッション1つを操作します。

```sh
cargo build --target-dir target -p rscu-mcp
```

DeepSeekHarness では、ローカルの stdio サーバーとして起動コマンドに `<リポジトリの絶対パス>/target/debug/rscu-mcp`、引数に `["--socket", "<rscu の操作ソケットの絶対パス>"]` を指定します。Harness 側と rscu は同じマシンで実行します。上の起動例に続けて Codex に登録する場合は、次のコマンドを使います。

```sh
codex mcp add rscu -- "$PWD/target/debug/rscu-mcp" --socket "$dir/control"
```

`windows` でウィンドウ ID を取得し、`screenshot` で PNG と画像 ID を受け取ります。`click` などのマウス操作には画像 ID と画像内の座標を渡します。撮影時に前回の画像 ID を `since` に指定すると、変化がない場合はメタデータだけを返します。`crop` で撮影範囲も指定できます。

## 検証

GTK 4 の Python バインディング、Pillow、xterm、xev、Xwayland がある環境では、次の操作テストを実行できます。

```sh
python3 tests/smoke.py
python3 tests/smoke.py --renderer gles
python3 tests/mcp.py
```

Pixman と GLES で、PNG の画素と部分撮影、複数ウィンドウの切り替え、最大化と復元、Wayland と X11 のメニューとモーダル、キーとマウス入力、停止時のソケット削除を検証しました。

検証機の Ubuntu 22.04 と XWayland 22.1.1 では、NVIDIA RTX 6000 Ada での GLES 合成と、専用 XWayland での NVIDIA GLX を確認しました。nanoem の起動、PNG 撮影、最大化と復元、指定座標へのクリック、対象ウィンドウへのキーイベントまで確認しました。検証前後で通常デスクトップのアクティブウィンドウ ID は同じでした。

検証機では `検証用ディレクトリ` の検証用ビルドで、nanoem の File → Save Project As と File → Open Project を操作しました。もとのラボ版ではファイル選択が無効だったため、検証用コピーで `NANOEM_ENABLE_IMGUI_FILE_DIALOG=ON` に設定しています。このビルドは起動直後にメニューバーが撮影範囲から外れる場合があるため、検証時は最大化してから操作しました。撮影とログは同ディレクトリの `evidence/` にあります。

nanoem のメニューバーの同一領域を５回確認した試験では、通常の撮影は PNG ５枚、画像 ID を指定した場合は PNG １枚と `unchanged` ４件でした。この試験ではモデル入力トークン数を計測していません。

## RDP で閲覧

`run` に `--rdp-port 3390` を付けると、セッションの起動と同時に閲覧専用の RDP サーバーが待ち受けます。アプリのウィンドウをタイルで並べ、AI が操作しているタイルには青い枠を表示します。AI のクリック位置も表示します。タイルを左クリックすると拡大表示になります。上部の `< Tiles` ボタンか Esc で一覧に戻ります。閲覧画面の操作はアプリへのクリックやキー入力として扱われません。

```sh
target/debug/rscu --socket "$dir/control" run --rdp-port 3390 \
  --wayland weston-simple-shm --x11 xterm
```

同じ PC の Remmina では RDP で `127.0.0.1:3390` に接続します。認証画面が出たらユーザー名は `viewer`、パスワードとドメインは空欄で進みます。接続プロファイルの解像度はカスタムの 1280×720、セキュリティは TLS を選びます。証明書はセッションごとに生成されます。待ち受けはループバックに限定しています。

別 PC から見る場合は、手元の PC で SSH トンネルを開いてから Remmina で `127.0.0.1:13390` に接続します。

```sh
ssh -N -o ExitOnForwardFailure=yes -L 127.0.0.1:13390:127.0.0.1:3390 rscu-host
```

初版の表示は 1280×720、最大約 15 FPS、同時接続は１クライアントです。配信には RDP のビットマップ更新を使います。Remmina を閉じても rscu とアプリは起動したままで、再接続するとタイル一覧から表示されます。RDP を省いてビルドするには `cargo build --no-default-features` を使います。
