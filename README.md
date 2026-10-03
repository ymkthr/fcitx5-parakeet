# fcitx5-parakeet

[日本語](README.md) | [English](README.en.md)

fcitx5からNVIDIA Parakeetを使う、ローカル音声入力です。
常にfcitx5に読み込まれるモジュールとして動作し、トリガーキーを押すとその場で音声入力が始まります。
入力メソッド自体は切り替えません。

## インストール

Arch Linuxのほか、Debian/Ubuntu（`.deb`）とFedora（`.rpm`）向けのパッケージをビルドできます。fcitx5とPipeWireが動作している環境が前提です。

### AURから

```sh
paru -S fcitx5-parakeet   # または yay -S fcitx5-parakeet
```

パッケージにはモデルが含まれません。インストール後に、デスクトップのユーザーとして次を実行してください。

```sh
parakeetd-download-models
systemctl --user enable --now parakeetd.socket
busctl --user call org.fcitx.Fcitx5 /controller org.fcitx.Fcitx.Controller1 Restart
parakeet-capslock-menu apply   # GNOMEでCapsLockをMenuキーにする場合
```

### debまたはrpmから

Debian/UbuntuとFedoraでは、dockerまたはpodmanのあるマシンでパッケージをビルドしてインストールします。

```sh
packaging/build.sh deb    # packaging/dist/ に .deb ができる（既定はdebian:trixie）
packaging/build.sh rpm    # packaging/dist/ に .rpm ができる（既定はfedora:42）
sudo apt install ./packaging/dist/fcitx5-parakeet_*.deb
sudo dnf install ./packaging/dist/fcitx5-parakeet-*.rpm
```

インストール後の手順はAURと同じです。詳細は`packaging/README.md`を参照してください。

### リポジトリから

リポジトリのルートから次のスクリプトを実行してください。

```sh
./scripts/install.sh
```

このスクリプトは次の処理を行います。

1. Rust製デーモンとfcitx5アドオンをビルドしてインストールする。
2. 日本語モデル、英語モデル、Silero VADモデル、かな漢字変換モデルjinen-v2-smallをダウンロードする。
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
parakeet-capslock-menu revert
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

録音中はカーソル付近に`🎙️`と、それまでに話した内容の途中経過（末尾40文字）が表示されます。
途中経過は約0.5秒ごとに更新される表示用の文字列です。
入力される文章は、録音の終了後に録音をまとめて認識し直し、同音異義語を補正したものです。
認識処理中は`…`が表示されます。

長い音声入力は、話している途中でも少しずつ入力されます。
まだ入力していない録音が60秒を超えると、その後半で最も長い間（ま）までを同じ方法で認識して入力し、録音はそのまま続きます。
録音は合計600秒で自動的に終了し、残りの部分は録音を終了したときと同じように入力されます。

録音中にEscapeを押すと、まだ入力されていない部分を取り消します。
それ以外のキーは、現在の入力メソッドで通常どおり入力できるので、音声入力の直後でもすぐに編集できます。

録音中に通知や画面の切り替えで別のウィンドウへフォーカスが移った場合は、録音を終了して認識します。
認識結果は、元の入力欄にフォーカスが戻ったときに入力されます。

変換中の未確定文字列がある間は、トリガーキーを押しても音声入力を開始せず、先に確定するようカーソル付近に表示します。

認識言語は既定で`auto`です。
日本語と英語の両方を認識でき、どちらの言語かはデーモンが判定します。
設定で`ja`または`en`に固定することもできます。

ログイン後最初の音声入力は、デーモンが初回にモデルを読み込むため、数秒余分に時間がかかることがあります。

## 日本語の同音異義語補正

日本語の認識結果は「機会」と「機械」のような同音異義語を取り違えることがあります。
デーモンは認識結果を読みに戻し、かな漢字変換モデル[jinen-v2-small](https://huggingface.co/togatogah/jinen-v2-small.gguf)で変換し直します。
このときカーソルより前にある最大64文字を文脈として使います。
文脈はアプリが周辺テキストを提供している場合だけ送られ、ログには残りません。
長い音声入力の途中で入力した部分も、続く部分の文脈になります。

変換結果のほうが認識結果よりモデルにとって明らかにもっともらしい場合だけ、認識結果を置き換えます。
長い文章は文ごとに区切って補正し、前の文を文脈として使います。
合成音声で読み上げた96件の短い発話では3件の誤りが直り、10秒から2分の長い発話22件では正しかった語が書き換えられたものはありませんでした。
補正にかかる時間は1文あたり約50ミリ秒です（4スレッドのCPU）。

モデル（約80MB）は`parakeetd-download-models`（リポジトリでは`scripts/download-models.sh`）が`~/.local/share/parakeetd/models/jinen-v2-small/`へダウンロードします。
モデルファイルがなければ補正は無効になり、認識結果をそのまま入力します。
補正にはAVX2、FMA、F16C、BMI2に対応したCPU（Intel Haswell、AMD Excavator以降）が必要で、対応していないCPUでは補正を自動で無効にします。

補正を止めるには、`~/.config/parakeetd/config.toml`に次を書きます。

```toml
[correction]
enabled = false
```

`margin`は置き換えに必要な対数尤度の差（nat単位、既定値6.0）です。
大きくすると置き換えが減り、小さくすると増えます。

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
| ShowStatus | True | カーソル付近へ`🎙️`と途中経過、`…`を表示する |

マイクを変更する場合は`~/.config/parakeetd/config.toml`を作成し、PipeWireのソース名を指定します。
設定例は`daemon/config.example.toml`にあります。

```toml
target = "alsa_input.example"
```

指定したマイクが外れている間は既定のマイクで録音し、再び接続されると指定したマイクに戻ります。

途中経過の更新間隔は`partial_interval_ms`（ミリ秒、既定値500）で変えられます。
0にすると途中経過を表示しません。

```toml
partial_interval_ms = 0
```

長い音声入力で途中の部分を入力するまでの録音の長さは`commit_after_seconds`（秒、既定値60）で変えられます。
間のない発話は`max_seconds`（秒、既定値120）に達した時点で、最も静かなところで区切ります。
録音が自動的に終了するまでの長さは`max_recording_seconds`（秒、既定値600）です。

```toml
commit_after_seconds = 30
max_recording_seconds = 900
```

設定変更後はデーモンを再起動してください。

```sh
systemctl --user restart parakeetd.service
```

ログは次のコマンドで確認できます。

```sh
journalctl --user -u parakeetd.service -f
```

## ライセンス

fcitx5-parakeet本体はMITライセンスです（`LICENSE`）。

パッケージには音声認識ライブラリとして[sherpa-onnx](https://github.com/k2-fsa/sherpa-onnx)（Apache License 2.0、`LICENSE-APACHE-2.0`）と[ONNX Runtime](https://github.com/microsoft/onnxruntime)（MITライセンス）の共有ライブラリを同梱しています。

インストール時にダウンロードするNVIDIA Parakeetの音声認識モデルは[CC BY 4.0](https://creativecommons.org/licenses/by/4.0/)、Silero VADのモデルはMITライセンス、jinen-v2-smallは[CC BY-SA 4.0](https://creativecommons.org/licenses/by-sa/4.0/)で配布されています。
モデルはパッケージには含まれません。

デーモンにはかな漢字変換の推論に使う[llama.cpp](https://github.com/ggml-org/llama.cpp)（MITライセンス）と、読みを求めるためのIPADIC辞書（`licenses/ipadic.LICENSE`）を組み込んでいます。

同梱物と依存物の一覧は`THIRD_PARTY_NOTICES.md`にあります。
