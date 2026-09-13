# パッケージング

- `arch/` — 作業ツリーからビルドする開発用PKGBUILD。`scripts/install.sh`が使う。
- `aur/` — AURに公開するPKGBUILD。GitHubのタグ付きtarballからビルドする。
- `deb/debian/` — Debian/Ubuntu向けのdebhelperパッケージング。
- `rpm/` — Fedora向けのspecファイル。
- `stage.sh` — ビルド済みのデーモンとアドオンをパッケージルートへ配置する共通スクリプト。debとrpmが使う。
- `build.sh` — コンテナ内で`.deb`または`.rpm`をビルドする。成果物は`dist/`へ出る。

## debとrpmのビルド

dockerまたはpodmanが必要です。作業ツリー（gitが追跡しているファイルと、無視されていない未追跡ファイル）をコンテナへ渡してビルドします。

```sh
packaging/build.sh deb                 # debian:trixie
packaging/build.sh deb ubuntu:24.04    # イメージを指定
packaging/build.sh rpm                 # fedora:42
```

sherpa-onnxのリリースアーカイブは`packaging/cache/`にキャッシュされます。
バージョンを上げるときは`daemon/Cargo.toml`、`fcitx5/CMakeLists.txt`、各PKGBUILDに加えて`deb/debian/changelog`と`rpm/fcitx5-parakeet.spec`も揃えます。

ビルドしたパッケージのインストールは通常どおりです。

```sh
sudo apt install ./packaging/dist/fcitx5-parakeet_0.3.0-1_amd64.deb
sudo dnf install ./packaging/dist/fcitx5-parakeet-0.3.0-1.fc42.x86_64.rpm
```

インストール後の手順（モデルの取得、socketの有効化、fcitx5の再起動）はAURと同じです。README.mdを参照してください。

## AURへの公開手順

1. `daemon/Cargo.toml`、`fcitx5/CMakeLists.txt`、`aur/PKGBUILD`の`pkgver`を同じ版に揃える。
2. masterに取り込んだ後、タグを打って送る。

   ```sh
   git tag v0.3.0 && git push origin v0.3.0
   ```

3. `aur/`でチェックサムと`.SRCINFO`を更新し、ビルドを確認する。

   ```sh
   cd packaging/aur
   updpkgsums
   makepkg --printsrcinfo > .SRCINFO
   makepkg -sf
   ```

4. AURリポジトリへ`PKGBUILD`、`.SRCINFO`、`fcitx5-parakeet.install`の3ファイルを送る。
   初回は`ssh://aur@aur.archlinux.org/fcitx5-parakeet.git`をcloneして空リポジトリを作る。
