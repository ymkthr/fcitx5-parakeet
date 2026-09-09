# fcitx5-parakeet

[日本語](README.md) | [English](README.en.md)

fcitx5からNVIDIA Parakeetを使う、ローカル音声入力です。
常にfcitx5に読み込まれるモジュールとして動作し、トリガーキーを押すとその場で音声入力が始まります。
入力メソッド自体は切り替えません。

## インストール

Arch Linuxで利用できます。
fcitx5とPipeWireが動作している環境で、リポジトリのルートから次のスクリプトを実行してください。

```sh
./scripts/install.sh
```

このスクリプトは次の処理を行います。

1. Rust製デーモンとfcitx5アドオンをビルドしてインストールする。
2. 日本語モデル、英語モデル、Silero VADモデルをダウンロードする。
3. ユーザー用systemd socketを有効にする。
4. fcitx5が動作していれば再起動して、新しいモジュールを読み込む。
5. GNOMEでは、CapsLockをMenuキーとして扱うXKBオプション`caps:menu`を追加する。

ビルドには`base-devel`、`cargo`、`clang`、`cmake`、`extra-cmake-modules`、`gettext`が必要です。
不足しているパッケージは`makepkg`がインストールを確認します。
音声認識モデルは`~/.local/share/parakeetd/models/`へ保存されます。

インストール時にfcitx5が停止していた場合は、手動でfcitx5を起動してください。

インストール後はデーモンとの接続を確認できます。

```sh
parakeet-ctl hello
parakeet-ctl status
```

## 使い方

既定のトリガーキーは`Menu`で、CapsLockをMenuキーとして使います。
WaylandではfcitxがCapsLockの状態をオフへ戻せないため、キーを横取りするのではなく、OS側でMenuキーへ読み替えます。

GNOMEでは、インストーラーがこの読み替えを自動で設定します。
既存のXKBオプションは残したまま`caps:menu`を追加し、すでに`caps:`で始まるオプションがある場合は競合するため変更しません。
読み替え後は、大文字を固定するCapsLock本来の機能は使えなくなります。

CapsLockをそのまま残す場合は、インストーラーに`--keep-capslock`を付けます。
その場合はキーボードのMenuキーを使うか、設定でトリガーキーを変更してください。

```sh
./scripts/install.sh --keep-capslock
```

読み替えを元に戻すには次のコマンドを実行します。
`caps:menu`だけを取り除き、他のオプションは残します。

```sh
./scripts/capslock-menu.sh revert
```

GNOME以外の環境では、インストーラーは設定を変更せず手順を表示します。
X11デスクトップでは次のコマンドで読み替えられます（ログアウトまで有効）。

```sh
setxkbmap -option caps:menu
```

KDEの場合はシステム設定からキーボードの設定を開き、キーボードショートカット、CapsLockの動作の順に進み、CapsLockを追加のMenuキーとして扱う設定を選びます。

入力欄でCapsLockを押し、カーソル付近にマイクの印が出れば、キーはfcitx5まで届いています。
印が出ない場合は、キーボードのファームウェアやキー割り当てツールがCapsLockを別のキーに変えていないか確認してください。

トリガーキーを短く（250ミリ秒未満）押すと、録音が開始したまま維持されます。
もう一度押すと録音が終了し、認識結果がカーソル位置へ入力されます。

トリガーキーを長押しすると、押している間だけ録音します。
離すと録音が終了し、認識結果がカーソル位置へ入力されます。

録音中はカーソル付近に`🎙️`が表示されます。
認識処理中は`…`が表示されます。

録音中にEscapeを押すと録音を取り消します。
それ以外のキーは、現在の入力メソッドで通常どおり入力できるので、音声入力の直後でもすぐに編集できます。

変換中の未確定文字列がある間は、トリガーキーを押しても音声入力を開始しません。

認識言語は既定で`auto`です。
日本語と英語の両方を認識でき、どちらの言語かはデーモンが判定します。
設定で`ja`または`en`に固定することもできます。

ログイン後最初の音声入力は、デーモンが初回にモデルを読み込むため、数秒余分に時間がかかることがあります。

## 設定

`fcitx5-configtool`を開き、アドオン（Addons）から「Parakeet Voice Input」を選ぶと設定できます。
設定ファイルは`~/.config/fcitx5/conf/parakeet.conf`です。

| 項目 | 既定値 | 内容 |
| --- | --- | --- |
| TriggerKey | Menu | 録音の開始と終了に使うキー |
| CancelKey | Escape | 録音中に押すと録音を取り消すキー |
| TapThresholdMs | 250 | この時間より短い押下は録音を維持したままロックし、長い押下は押している間だけ録音する |
| Language | auto | auto、ja、enのいずれか |
| SocketPath | 空 | 空の場合は`$XDG_RUNTIME_DIR/parakeetd.sock`を使う |
| ShowStatus | True | カーソル付近へ`🎙️`や`…`を表示する |

マイクを変更する場合は`~/.config/parakeetd/config.toml`を作成し、PipeWireのソース名を指定します。
設定例は`daemon/config.example.toml`にあります。

```toml
target = "alsa_input.example"
```

設定変更後はデーモンを再起動してください。

```sh
systemctl --user restart parakeetd.service
```

ログは次のコマンドで確認できます。

```sh
journalctl --user -u parakeetd.service -f
```
