# パッケージング

- `arch/` — 作業ツリーからビルドする開発用PKGBUILD。`scripts/install.sh`が使う。
- `aur/` — AURに公開するPKGBUILD。GitHubのタグ付きtarballからビルドする。

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
