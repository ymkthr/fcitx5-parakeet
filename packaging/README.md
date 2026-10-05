# パッケージング

- `arch/` — 作業ツリーからビルドする開発用PKGBUILD。`scripts/install.sh`が使う。
- `aur/` — AURに公開するPKGBUILD。GitHubのタグ付きtarballからビルドする。
- `deb/debian/` — Debian/Ubuntu向けのdebhelperパッケージング。
- `rpm/` — Fedora向けのspecファイル。
- `stage.sh` — ビルド済みのデーモンとアドオンをパッケージルートへ配置する共通スクリプト。debとrpmが使う。
- `build.sh` — コンテナ内で`.deb`または`.rpm`をビルドする。成果物は`dist/`へ出る。
- `obs/` — openSUSE Build Service（OBS）に置くファイル。`assemble.sh`がspecとdebianディレクトリから組み立てる。

## debとrpmのビルド

dockerまたはpodmanと、ホストの`cargo`が必要です。作業ツリー（gitが追跡しているファイルと、無視されていない未追跡ファイル）をコンテナへ渡してビルドします。

```sh
packaging/build.sh deb                 # debian:trixie
packaging/build.sh deb ubuntu:24.04    # イメージを指定
packaging/build.sh rpm                 # fedora:44
```

ビルドはOBSと同じくネットワークなしで動きます。ビルド依存だけをネットワークありで入れた一時イメージを作り、その中で`--network none`でビルドします。
ビルドに使う外部の入力は、事前に`packaging/cache/`へ用意してキャッシュします。sherpa-onnxのリリースアーカイブ、`lindera-ipadic`が本来ビルド中にダウンロードするIPADIC辞書（約13MB）、`cargo vendor`したクレート（`vendor.tar.xz`。`daemon/Cargo.lock`が更新されると作り直す）の3つです。
デーモンのビルドにはRust 1.88以上が必要です。Debian trixieでは`rustc-web`と`cargo-web`を使い、それより古いRustしかないイメージではビルドできません。
linderaを上げたときは、`deb/debian/rules`と`rpm/fcitx5-voice-ja.spec`にあるIPADICのキャッシュディレクトリ名（`<lindera-ipadicの版>-fmt<辞書形式の版>`）も合わせます。ずれているとオフラインのビルドがダウンロードを試みて失敗します。
バージョンを上げるときは`daemon/Cargo.toml`、`fcitx5/CMakeLists.txt`、各PKGBUILDに加えて`deb/debian/changelog`と`rpm/fcitx5-voice-ja.spec`も揃えます。

ビルドしたパッケージのインストールは通常どおりです。

```sh
sudo apt install ./packaging/dist/fcitx5-voice-ja_0.4.1-1_amd64.deb
sudo dnf install ./packaging/dist/fcitx5-voice-ja-0.4.1-1.fc44.x86_64.rpm
```

インストール後の手順（モデルの取得、socketの有効化、fcitx5の再起動）はAURと同じです。README.mdを参照してください。

## AURへの公開手順

1. `daemon/Cargo.toml`、`fcitx5/CMakeLists.txt`、`aur/PKGBUILD`の`pkgver`を同じ版に揃える。
2. masterに取り込んだ後、タグを打って送る。

   ```sh
   git tag v0.4.1 && git push origin v0.4.1
   ```

3. `aur/`でチェックサムと`.SRCINFO`を更新し、ビルドを確認する。

   ```sh
   cd packaging/aur
   updpkgsums
   makepkg --printsrcinfo > .SRCINFO
   makepkg -sf
   ```

4. AURリポジトリへ`PKGBUILD`、`.SRCINFO`、`fcitx5-voice-ja.install`の3ファイルを送る。
   初回は`ssh://aur@aur.archlinux.org/fcitx5-voice-ja.git`をcloneして空リポジトリを作る。

## OBSへの公開手順

OBSのプロジェクトは[`home:ymkthr:fcitx5-voice-ja`](https://build.opensuse.org/package/show/home:ymkthr:fcitx5-voice-ja/fcitx5-voice-ja)です。Fedora 43、Fedora 44、Debian 13向けにビルドし、`download.opensuse.org`からdnfとaptのリポジトリとして配信します。
OBSのビルドはネットワークを使えません。リリースのtarball、sherpa-onnx、IPADICはOBSが`_service`の`download_files`で取得します。公開OBSには`cargo_vendor`サービスがないため、クレートだけは手元で`vendor.tar.xz`に固めて上げます。

1. AURと同じくタグを打って送る。specの`Source0`はそのタグのtarballを指す。
2. `osc`でパッケージをチェックアウトし、`assemble.sh`でファイルを置き換えてコミットする。

   ```sh
   osc co home:ymkthr:fcitx5-voice-ja fcitx5-voice-ja
   cd home:ymkthr:fcitx5-voice-ja/fcitx5-voice-ja
   ~/path/to/fcitx5-voice-ja/packaging/obs/assemble.sh .
   osc addremove && osc ci -m "Update to 0.4.1"
   ```

3. `osc results`ですべてのディストリが`succeeded`になるのを確かめる。

Debian向けはOBSの`debtransform`が.dscからソースパッケージを組み立てます。`debtransform`はソースのtarballを1つしか受け付けないため、sherpa-onnx、`vendor.tar.xz`、IPADICは.dscの`DEBTRANSFORM-FILES`でツリーの最上位に置き、`debian/rules`がそこから展開します。
プロジェクトのリポジトリは、Fedoraが`Fedora:<版>/update`、Debianが`Debian:13/security`と`Debian:13/update`を参照します。Debianの標準リポジトリだけではRustが1.88より古く、ビルドできません。
