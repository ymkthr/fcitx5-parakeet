# fcitx5-parakeet

[日本語](README.md) | [English](README.en.md)

fcitx5からNVIDIA Parakeetを使う、ローカル音声入力です。
日本語、英語、日英自動判別の入力メソッドを提供します。

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
4. 現在のfcitx5入力メソッドグループへParakeetを追加する。

ビルドには`base-devel`、`cargo`、`clang`、`cmake`、`extra-cmake-modules`、`gettext`が必要です。
不足しているパッケージは`makepkg`がインストールを確認します。
音声認識モデルは`~/.local/share/parakeetd/models/`へ保存されます。

fcitx5が停止していた場合は、fcitx5を起動してから入力メソッドを登録してください。

```sh
python3 scripts/fcitx5-register.py
```

インストール後はデーモンとの接続を確認できます。

```sh
parakeet-ctl hello
parakeet-ctl status
```

## 使い方

1. fcitx5の入力メソッド切替キーで「Parakeet 音声入力」を選びます。
2. Spaceを押し続けます。
3. カーソル付近に`🎙️`が表示されたら話します。
4. Spaceを離すと音声認識が始まり、結果がカーソル位置へ入力されます。

Spaceを250ミリ秒未満だけ押した場合は、録音せず通常のスペースを入力します。
録音中にEscapeを押すと録音を取り消します。
SpaceとEscape以外のキーは通常どおり入力できます。

利用する入力メソッドによって認識言語が変わります。

| 入力メソッド | 認識言語 |
| --- | --- |
| Parakeet 音声入力（自動、日英） | 日本語または英語を自動判別 |
| Parakeet 音声入力（日本語） | 日本語 |
| Parakeet 音声入力（英語） | 英語 |

通常は「自動、日英」を使用します。
言語を固定したい場合は日本語または英語の入力メソッドへ切り替えてください。

## 設定

`fcitx5-configtool`でParakeet入力メソッドの設定を開きます。
設定ファイルは`~/.config/fcitx5/conf/parakeet.conf`です。

| 項目 | 既定値 | 内容 |
| --- | --- | --- |
| 録音の開始方法 | Push to talk | Toggleにすると、Spaceを押すたびに録音開始と終了を切り替える |
| 録音キー | Space | 録音の開始と終了に使うキー |
| キャンセルキー | Escape | 録音を取り消すキー |
| TapThresholdMs | 250 | 通常のキー入力として扱う押下時間の上限 |
| SocketPath | 空 | 空の場合は`$XDG_RUNTIME_DIR/parakeetd.sock`を使う |
| ShowStatus | True | カーソル付近へ録音状態を表示する |

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
