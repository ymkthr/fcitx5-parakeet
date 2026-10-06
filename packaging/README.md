# パッケージング

各ディレクトリとスクリプトの用途は次のとおりです。

- `arch/`: 作業ツリーからビルドする開発用の`PKGBUILD`です。`scripts/install.sh`が使います。
- `aur/`: AURへ公開する`PKGBUILD`です。GitHubのタグアーカイブからビルドします。
- `deb/debian/`: DebianとUbuntu向けのdebhelper設定です。
- `rpm/`: Fedora向けのspecファイルです。
- `stage.sh`: ビルド済みのデーモンとアドオンをパッケージルートへ配置します。debとrpmが使います。
- `build.sh`: コンテナ内で`.deb`または`.rpm`をビルドします。成果物は`dist/`へ出ます。
- `obs/`: openSUSE Build Service（OBS）へ送るファイルと公開ツールです。
- `release.py`: リリースバージョンの検査と更新を一括して行います。

## debとrpmをローカルでビルドする

DockerまたはPodmanと、ホストの`cargo`が必要です。ビルドには、Gitが追跡しているファイルと、無視されていない未追跡ファイルを渡します。

```sh
packaging/build.sh deb
packaging/build.sh deb ubuntu:24.04
packaging/build.sh rpm
```

ビルドはOBSと同じくネットワークなしで動きます。最初にネットワークありの一時イメージへビルド依存を入れ、その後のビルドでは`--network none`を指定します。

外部の入力は事前に`packaging/cache/`へ置きます。対象はsherpa-onnxのリリースアーカイブ、`lindera-ipadic`が使うIPADIC辞書、`cargo vendor`が作る`vendor.tar.xz`です。`daemon/Cargo.lock`を更新した場合は`vendor.tar.xz`も作り直してください。

デーモンのビルドにはRust 1.88以上が必要です。Debian trixieでは`rustc-web`と`cargo-web`を使えます。これより古いRustしかないイメージではビルドできません。

linderaを更新した場合は、`deb/debian/rules`と`rpm/fcitx5-voice-ja.spec`にあるIPADICのキャッシュディレクトリ名も更新してください。この名前は`<lindera-ipadicのバージョン>-fmt<辞書形式のバージョン>`です。値が一致しないと、オフラインビルドがダウンロードを試して失敗します。

ビルドしたパッケージは通常のパッケージ管理コマンドでインストールできます。実際の値を`VERSION`、`REVISION`、`FEDORA`の各環境変数へ設定してから実行します。

```sh
sudo apt install "./packaging/dist/fcitx5-voice-ja_${VERSION}-${REVISION}_amd64.deb"
sudo dnf install "./packaging/dist/fcitx5-voice-ja-${VERSION}-${REVISION}.fc${FEDORA}.x86_64.rpm"
```

インストール後の手順はAURパッケージと同じです。モデルを取得し、socketを有効にして、fcitx5を再起動します。詳しくはリポジトリ直下の`README.md`を参照してください。

## ラベルを付けてマージするとリリースが始まる

`master`へマージするプルリクエストに、次のラベルを1つ付けます。

- `release:major`: メジャーバージョンを上げます。
- `release:minor`: マイナーバージョンを上げます。
- `release:patch`: パッチバージョンを上げます。

ラベルを付けたプルリクエストをマージすると、`.github/workflows/tag-release.yml`が動きます。リリースバージョンはプルリクエスト内で手作業によって変更しないでください。

リリースラベルがないプルリクエストは、マージしてもリリースされません。複数のリリースラベル、または未定義の`release:*`ラベルがある場合はワークフローが失敗します。マージされずに閉じた場合は何も変更しません。

### 第1段階でバージョン、コミット、タグを同時に確定する

`tag-release.yml`は常に現在の`master`から処理を始めます。プルリクエストのheadはチェックアウトせず、実行もしません。

ワークフローは次のファイルにあるバージョンが一致することを確認します。

- `daemon/Cargo.toml`
- `daemon/Cargo.lock`の`voice-jad`ブロック
- `fcitx5/CMakeLists.txt`
- `packaging/arch/PKGBUILD`
- `packaging/aur/PKGBUILD`
- `packaging/aur/.SRCINFO`
- `packaging/deb/debian/changelog`
- `packaging/rpm/fcitx5-voice-ja.spec`

現在のバージョンが最大の正規タグと一致した場合だけ、指定された種類のバージョンを上げます。パッケージリビジョンは1へ戻します。DebianとRPMの変更履歴にはプルリクエスト番号とマージコミットの時刻だけを記録します。プルリクエストのタイトルと本文は取り込みません。

バージョンコミットと軽量タグは`git push --atomic`で同時に送ります。別のリリースが先に`master`を更新した場合は、最新の状態からバージョンを計算し直します。同じプルリクエストを再実行した場合は、コミットの`Release-PR`、`Release-Merge-SHA`、`Release-Kind`、`Release-Previous-Version`トレーラーを検出します。この場合はバージョンを上げず、既存のタグを第2段階へ再送します。

### 第2段階でGitHub ReleaseとOBSを公開する

`publish-release.yml`は第1段階から`repository_dispatch`を受け取ります。`GITHUB_TOKEN`でタグを送ってもタグpushのワークフローは起動しないため、この通知が必要です。

第2段階はタグの形式、`master`からの到達性、コミットトレーラー、リポジトリ内のバージョンを改めて検査します。その後、GitHubが生成したタグアーカイブをダウンロードし、リンクや危険なパスがないことを確認します。

OBS向けのソース作成ジョブには秘密情報を渡しません。タグのコミット時刻を`SOURCE_DATE_EPOCH`に設定し、次の5ファイルを作ります。

- `_service`
- `fcitx5-voice-ja.spec`
- `fcitx5-voice-ja.dsc`
- `debian.tar.xz`
- `vendor.tar.xz`

ワークフローは5ファイルのSHA-256マニフェストを作り、有効期間が30日のActions artifactとして渡します。GitHub Releaseが既にある場合は再利用します。ない場合は自動生成したリリースノート付きで作成します。

OBSへの送信ジョブだけが`release` Environmentを使います。このジョブはartifactのファイル名、通常ファイルであること、SHA-256を確認してから`packaging/obs/publish.py`を実行します。転送されたファイルは展開も実行もしません。OBS側が直前のバージョンなら、そのビルドの終了を待ってから更新します。同じバージョンなら差分だけを照合し、`osc status`が空ならコミットしません。新しいバージョンが先に公開済みの場合は、完了済みとしてダウングレードを行いません。

OBSの対象は`Fedora_43/x86_64`、`Fedora_44/x86_64`、`Debian_13/x86_64`です。3対象すべてでリポジトリが`published`、パッケージが`succeeded`になり、要求した`X.Y.Z-1`のパッケージファイルが現れるまで待ちます。後続リリースが先にOBSを更新した場合は、後続側がこの条件を確認済みのため、先行側も完了として終了します。

OBSのプロジェクトは[`home:ymkthr:fcitx5-voice-ja`](https://build.opensuse.org/package/show/home:ymkthr:fcitx5-voice-ja/fcitx5-voice-ja)です。OBSのビルドではネットワークを使いません。リリースアーカイブ、sherpa-onnx、IPADICは`_service`の`download_files`で取得します。公開OBSには`cargo_vendor`サービスがないため、Rustクレートは`vendor.tar.xz`として送ります。

プロジェクトのリポジトリは、Fedoraが`Fedora:<バージョン>/update`、Debianが`Debian:13/security`と`Debian:13/update`を参照します。Debianの標準リポジトリだけではRustが1.88より古く、ビルドできません。

## GitHubのEnvironmentとラベルを初回だけ設定する

リポジトリのSettingsで`release`というEnvironmentを作ります。次のEnvironment secretsを登録してください。

- `OBS_USERNAME`: `home:ymkthr:fcitx5-voice-ja/fcitx5-voice-ja`のソースを更新できるOBSユーザーです。
- `OBS_PASSWORD`: そのユーザーのパスワードです。

Environmentのdeployment branchesには`master`だけを許可します。承認を挟む場合はrequired reviewersも設定します。OBSの認証情報はRepository secretsへ重複して登録しないでください。ソース作成ジョブにはEnvironmentを指定していないため、この認証情報を読み取れません。

GitHubのIssues設定では、`release:major`、`release:minor`、`release:patch`の3ラベルを作ります。表記は完全一致させてください。

## 失敗した自動リリースを再開する

第1段階を再開する場合は、マージ済みプルリクエストの番号を`PR_NUMBER`環境変数へ設定します。

```sh
gh workflow run tag-release.yml -f pr_number="$PR_NUMBER"
```

第2段階だけを再開する場合は、既存のタグを`TAG`環境変数へ設定します。

```sh
gh workflow run publish-release.yml -f tag="$TAG"
```

第1段階の再実行は、既存のバージョンコミットを検出して同じタグを再送します。第2段階の再実行は、既存のGitHub Releaseを使い、OBSのソースに差分がある場合だけコミットします。タグや`master`をforce pushして復旧しないでください。

## AURへの公開は手動で行う

自動リリースはAURリポジトリへpushしません。GitHubのタグとReleaseが作成された後、公開するタグを`TAG`環境変数へ設定し、そのツリーからAUR用ファイルを確認します。

```sh
git switch --detach "$TAG"
cd packaging/aur
updpkgsums
makepkg --printsrcinfo >.SRCINFO
makepkg -sf
```

確認後、AURリポジトリへ`PKGBUILD`、`.SRCINFO`、`fcitx5-voice-ja.install`を送ります。初回だけ`ssh://aur@aur.archlinux.org/fcitx5-voice-ja.git`をcloneしてリポジトリを用意してください。

OBSでは`debtransform`が`.dsc`からDebianソースパッケージを組み立てます。`debtransform`が受け取れるソースtarballは1つです。そのため、sherpa-onnx、`vendor.tar.xz`、IPADICは`.dsc`の`DEBTRANSFORM-FILES`で展開ツリーの最上位へ置きます。`debian/rules`はその場所から各アーカイブを読みます。
