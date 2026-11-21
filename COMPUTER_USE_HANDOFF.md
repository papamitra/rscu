# Linux Computer Use 引き継ぎ

2026-11-21 JST。ここでのCUはComputer Useを指す。検討段階で、専用環境の実装や導入はまだ行っていない。

現在の方針は [Linux Computer Use 設計案](COMPUTER_USE_DESIGN.md) を参照。以下は Weston を起点に検討した時点の記録。

## 目的と対象範囲

AIが専用デスクトップでGUIアプリを操作し、人間は普段のデスクトップで作業を続ける。第一候補はnanoemのように、AI用に起動し直せるアプリ。既存のユーザーデスクトップに開いている任意のウィンドウを操作する要件は別に扱う。

Waylandでは、既存デスクトップの画面取得や入力はコンポジタが管理する。ScreenCastとRemoteDesktopポータルは共通の枠組みを提供するが、利用できる機能はバックエンドによる。niriとGNOMEでフォーカスを変えずにクリックできる範囲の違いは、この検討では実機比較していない。

## 専用デスクトップ案

```text
人間 → 普段のデスクトップ
AI   → スクリーンショット・入力API
           ↕
       headless Waylandコンポジタ → 仮想画面
           ├─ ネイティブWaylandアプリ
           └─ XWayland → X11アプリ
```

AI用のアプリを専用ソケットにつないで起動する。コンポジタの仮想画面からスクリーンショットを取り、クリック・キー入力は仮想seat経由で送る。AI側でフォーカスが変わっても、人間が使うセッションからは独立している。専用環境の画面取得と入力をコンポジタ内で実装するなら、既存デスクトップを共有するポータルは不要。同じウィンドウで人間とAIが作業したい場合には、入力の競合や操作権の切り替えが別途必要になる。

今のCodexセッションで使えるブラウザ操作とは接続口が異なる。専用環境をAIから操作するには、画像を返すスクリーンショット機能と入力APIをエージェントのツールとして接続する。

## GPUとXWayland

物理モニターなしでもGPU描画は可能。対応ドライバーではDRM render nodeを使って表示モードを設定せずに描画できる。ただし、コンポジタの合成と、アプリ自身のOpenGL描画は別々にGPU使用を確認する。

検証機の現行nanoemはX11の `DISPLAY` を使い、通常のX11セッションではNVIDIAのGLX描画を確認済み。このビルドをWaylandの仮想画面内で動かすならXWaylandが必要。ネイティブWaylandアプリは直接接続できる。Xvfbで画面と入力だけを先に試す選択肢もあるが、NVIDIA GPU描画まで同じように使えるかは未確認。

## 次の検証

1. Westonのheadlessバックエンドで仮想画面を一つ起動し、GLレンダラーが実際にGPUを使うか確かめる。
2. 専用ソケットにテストアプリを起動し、スクリーンショット取得とクリック・キー入力を通す。既存デスクトップのフォーカスに影響しないことも確認する。
3. XWayland経由の `glxinfo -B` でレンダラーを確認し、nanoemの起動・描画・操作を試す。コンポジタ側でGPUが使えても、アプリ側で `llvmpipe` に落ちる場合がある。
4. Westonで画面取得・入力の接続が扱いづらければ、Smithayで必要な機能に絞った専用コンポジタを検討する。headless Weston＋XWayland＋NVIDIAの組み合わせは検証機で未検証。

## 参照

- [Westonのバックエンドとレンダラー](https://wayland.pages.freedesktop.org/weston/toc/running-weston.html)
- [Smithayの描画・入力基盤](https://smithay.github.io/smithay/smithay/backend/index.html)
- [Linux DRMのrender node](https://www.kernel.org/doc/html/v6.12/gpu/drm-uapi.html)
- [ScreenCastポータル](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.ScreenCast.html)
- [RemoteDesktopポータル](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.RemoteDesktop.html)
