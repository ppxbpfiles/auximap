# auximap - PPx aux: path IMAP bridge

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/Rust-2024%20Edition-orange.svg)](https://www.rust-lang.org/)
[![Platform](https://img.shields.io/badge/Platform-Windows-lightgrey.svg)](#)

**auximap** は、ファイラー「Paper Plane xUI (PPx)」の画面上でメール（IMAP）をファイルのように扱い、メールボックス内のファイルやフォルダの確認・整理・移動・削除などを行うためのツールです。  
PPx の `aux:` パス機能を利用し、メールサーバー上のメールボックスを仮想フォルダとして一覧・操作できます（※おまけとして Emacs 用の連携スクリプト `auximap.el` も同梱しています）。

---

## 特長

1. **完全自己完結型（単一 EXE）**
   - Python などの外部ランタイムや外部 DLL は不要。`auximap.exe` 単体で動作します。
2. **高速なレスポンス**
   - Rust でネイティブコンパイルされており、PPc のキー操作に対して素早く応答します。
3. **オンデマンド取得（省データ）**
   - 一覧表示時はヘッダ（件名・差出人・日時・サイズ・UID）のみを取得。
   - 本文（`.eml` や整形テキスト）は閲覧時（またはコピー時）にのみ取得するため、通信量やディスク容量を抑えられます。
4. **一括取得と分割取得（スイッチ切り替え対応）**
   - 通常は一括取得（Bulk）で動作します。
   - メール件数が多い場合（1,000〜5,000件等）でタイムアウトを防ぎたい場合は、スイッチ `--chunk` を付与することで 250件ずつの小分け取得に切り替えられます。
5. **メール本文の自動デコード＆テンプレート整形**
   - （※現在このアーカイブに入っているCFGではテンプレート整形には対応していません。.eml がそのまま読める形式です）
   - Base64 / Quoted-Printable や ISO-2022-JP / UTF-8 を自動デコード。
   - HTML専用メールはテキスト化して表示可能。
   - `view_template.txt` で表示フォーマットをカスタマイズ可能（CLI の `read` コマンドや Emacs 連携等で使用）。
6. **PPc からのフォルダ作成・管理**
   - PPc 上で `K`（ディレクトリ作成）を押すことで、メールサーバー上に新しいメールフォルダを作成できます。
   - PPc 上で `R`（名前変更）を押すことで、メールフォルダの名前を変更できます（※メールメッセージ自体のリネームは安全のため対象外）。
7. **複数アカウント対応**
   - さくら、Gmail など複数のアカウントを `auximap.ini` で一括管理できます。
8. **日本語文字コード対応**
   - UTF-8、ISO-2022-JP（JIS）、Shift_JIS、EUC-JP の各 MIME エンコードに対応。
9. **安全なゴミ箱移動**
   - PPc で `Delete` キーを押すと、サーバー上の `Trash`（ゴミ箱）フォルダへ移動します。
10. **重複メール検出・整理（report コマンド）**
    - 送信日時（秒）・差出人・件名・サイズが一致する重複メールを検出し、一覧表示またはゴミ箱へ移動できます。全フォルダ横断検索にも対応。

---

## ファイル構成

```text
auximap/
  ├── auximap.exe           # コンパイル済み実行ファイル（本体）
  ├── auximap.ini.sample    # 設定ファイルサンプル
  ├── auximap.cfg           # PPx 設定ファイル（UTF-16LE BOM付き）
  ├── omake/
  │   └── emacs/
  │       ├── auximap.el        # Emacs 用連携スクリプト（サンプル）
  │       └── README_emacs.md   # auximap.el の説明書
  ├── view_template.txt     # メール本文表示テンプレート
  ├── Cargo.toml            # Rust プロジェクト設定
  ├── src/
  │    └── main.rs          # 全処理のソースコード
  └── README.md             # 本説明書
```

---

## 導入手順

### 1. 設定ファイルの準備
`auximap.ini.sample` を同じフォルダにコピーして `auximap.ini` にリネームし、メールアカウントの情報を設定します。

> [!IMPORTANT]
> **【重要】ファイルごとの文字コードの違いについて**
> - **`auximap.ini`**: 必ず **`UTF-8`**（BOMなし または BOM付き）で保存してください！  
>   （※Rust製ツールが読み込むため、UTF-16 や Shift-JIS で保存すると起動エラーになります）
> - **`auximap.cfg`**: 必ず **`UTF-16LE（BOM付き）`** で保存してください！  
> [!TIP]
> **【アカウント名（セクション名）の命名規則について】**  
> `auximap.ini` のセクション名（例: `[sakura]`, `[gmail]`）は、PPc 上で **仮想フォルダの名前そのもの** として画面表示およびパス解決に使用されます。  
> そのため、セクション名にメールアドレスそのもの（`@` や `.` を含む文字列）を書くのではなく、Windows のフォルダ名として安全な **半角英数字（例: `[sakura]`, `[work]` など）** を指定してください。メールアドレス自体は各セクション内の `user = ...` に記述します。

```ini
[DEFAULT]
limit = 100               ; 一覧で取得する最大件数（新しい順。1000〜5000なども指定可能）
offset = 0                ; 取得開始オフセット（最新から何通スキップするか。例: 3000）
trash_folder = Trash      ; デフォルトのゴミ箱名
timezone = local          ; タイムゾーン（local: PC日本時間換算[既定], server: サーバー時間, utc: 協定世界時）
date_source = internal    ; 日時取得元（internal: サーバー受信日時[既定], header: 送信日時Date:）

; メール本文表示のフォーマット設定
[VIEW]
template_file = view_template.txt
html_policy = text

; さくらのメール設定例（ドメイン自動解決により host 等は省略可能！）
[sakura]
user = info@example.sakura.ne.jp
; host = example.sakura.ne.jp  ; 独自ドメインの場合は明示的に指定
; pass =                       ; PPx連携（%*pass）運用の場合は空欄（ini平文運用の場合はパスワード記述）

; Gmail 設定例（Google アカウントのアプリ パスワードを使用）
[gmail]
user = your_address@gmail.com
pass = abcd efgh ijkl mnop
```

### 設定の優先順位と継承ルール

`auximap.ini` の各パラメータは、以下の優先順位で適用されます：

1. **【最優先】個別アカウントセクション（例: `[sakura]`, `[gmail]`）の明示的な記述**
   - 個別セクションに設定値（`host`, `port`, `ssl`, `trash_folder`, `limit`, `offset` 等）が書かれている場合は、常にそれが 100% 最優先されます。
2. **【第2優先】ドメイン自動解決ルール（`[RULE:...]`）による補完**
   - `host` 等が未指定の場合、`user`（メールアドレス）のドメインにマッチするルールから自動補完・展開されます。
3. **【第3優先】`[DEFAULT]` セクションの記述**
   - `limit`, `offset`, `trash_folder`, `timezone`, `date_source` 等の共通設定が自動継承されます。
4. **【第4優先】プログラム内部の規定値**（組み込みルール、`port = 993`, `ssl = true` 等）

---

## ドメイン自動解決ルール（Auto-configuration）

`auximap` は、メールアドレスのドメイン名から IMAP 接続先サーバーやゴミ箱フォルダを自動判定・補完する **Auto-configuration** 機能を搭載しています。

### 1. アカウント設定の自動補完
さくらの初期ドメイン（`*.sakura.ne.jp`）や Gmail、Yahoo! などでは、`host` や `port`、`trash_folder` を書かずに、`user`（メールアドレス）を指定するだけで即座に接続できます：

```ini
[sakura]
user = mytenant@example.sakura.ne.jp
; ↑ これだけで host = example.sakura.ne.jp, port = 993, ssl = true, trash_folder = Trash が自動設定されます。
```

### 2. 組み込みルール（事前定義済み）
以下のドメインは最初から内部ルールが登録されているため、ini へのルール記述も不要です：
- **Gmail**: `gmail.com`, `googlemail.com` -> `imap.gmail.com:993` (SSL), ゴミ箱: `[Gmail]/ゴミ箱`
- **Yahoo! メール**: `yahoo.co.jp`, `ymail.ne.jp` -> `imap.mail.yahoo.co.jp:993` (SSL), ゴミ箱: `Trash`
- **さくらのメール**: `*.sakura.ne.jp` -> `{domain}:993` (SSL), ゴミ箱: `Trash`
- **Outlook / Hotmail**: `outlook.com`, `hotmail.com`, `live.com` -> `outlook.office365.com:993` (SSL), ゴミ箱: `Deleted`
- **iCloud**: `icloud.com`, `me.com` -> `imap.mail.me.com:993` (SSL), ゴミ箱: `Deleted Messages`

### 3. 自社ドメインや独自ルールの追加
`auximap.ini` に `[RULE:<domain_pattern>]` セクションを追加することで、独自のドメイン解決ルールを自由に定義・カスタマイズできます：

```ini
; 自社サーバー（例: user@company.com -> mail.company.com）
[RULE:company.com]
host = mail.{domain}
port = 993
ssl = true
trash_folder = Trash

; 共通デフォルトルール
[RULE:DEFAULT]
host = mail.{domain}
port = 993
ssl = true
trash_folder = Trash
```

- **使用可能なテンプレート変数**:
  - `{domain}`: メールアドレスのドメイン部分（例: `company.com`）
  - `{user_part}`: `@` より前のユーザー名（例: `taro`）
  - `{user}`: メールアドレス全体（例: `taro@company.com`）

---

## 認証情報の管理と設定

`auximap` では、PPx ダイアログ連携（`%*pass` / `%*user`）と `auximap.ini` への直接記述に対応しています。通常は `auximap.ini` の `pass = `（および `user = `）を空欄にして運用します。

### 認証仕様と動作ルール

1. **PPx ダイアログによる動的補完**
   - `auximap.cfg` では各コマンドに `--pass="%*pass" --user="%*user"` を指定しています。
   - `auximap.ini` の `pass` や `user` が空欄のアカウントは、初回アクセス時に PPx の入力ダイアログが表示され、セッション中メモリ保持されます。

2. **ini 優先ルール**
   - `auximap.ini` に `user` や `pass` が記述されているアカウントは、ini の設定値が優先されます。
   - PPx のダイアログ（`%*pass`, `%*user`）に値が存在しても無視され、ini の記述が使用されます。
   - ini に固定設定したアカウントと、ダイアログで都度入力するアカウントを併用可能です。

3. **認証情報のクリア（`*clearauth`）**
   - PPx 上で `*clearauth` を実行することで、メモリ保持されたパスワードやユーザー名を初期化できます。

---

## 仮想エントリとオンデマンド実体化の仕様

PPc 上のリストに表示される `.eml` ファイルは、ローカルディスク上に保存された実体ファイルではなく、IMAP サーバーのヘッダ情報（UID・件名・日時・サイズ）から動的に生成された仮想エントリ（ListFile）です。

- **一覧取得時**: メールのヘッダ情報のみを取得してリストを構築します。メール本文や添付ファイルはローカルディスクに保存されません。
- **閲覧・実行時**: エントリを開いた（Enter）時点で該当メールの本文・データを取得し、一時フォルダ（`%temp%`）に `.eml` ファイルを実体化してビューアを起動します。
- **コピー・マーク抽出時**: PPx のコピー操作等を行うと、指定された出力先フォルダへ実体化された `.eml` ファイルが保存されます。

---

## PPc での使い方

### 1. PPx への配置と取り込み
1. PPx の `auxcmd` フォルダ内に以下をコピーします（他の場所に入れるときは適宜書き換えてください）：
   - `auximap.exe`
   - `auximap.ini`
   - `view_template.txt`
2. PPc から `auximap.cfg` を取り込みます：
   - 例: PPc で `*ppcust CA auximap.cfg` を実行。

### 2. メールを開く
- **アカウント一覧を開く**:
  ```ppx
  aux://S_auximap/
  ```
  PPc 上に `[D] gmail\` や `[D] sakura\` がディレクトリとして並びます。

- **特定のアカウントのフォルダ一覧を開く**:
  ```ppx
  aux://S_auximap/sakura/
  ```
  `[D] INBOX\` や `[D] Sent\` などが並びます。

- **メールボックスに直接ジャンプ**:
  ```ppx
  aux://S_auximap/gmail/INBOX
  ```
  直近のメール一覧が表示されます。

### 3. メールの操作
- **新規フォルダ作成**:
  アカウント階層（例: `aux://S_auximap/sakura/`）で **`K` キー** を押し、フォルダ名を入力して Enter を押すと、メールサーバー上にフォルダが新規作成されます。
- **複写（コピー）**:
  メールを選んで（または複数マークして）反対画面の別フォルダ（例: `aux://S_auximap/sakura/仕事`）へ **`C` キー** を押すと、サーバー上で別フォルダへメールが複製されます。
- **移動（ムーブ）**:
  メールを選んで反対画面の別フォルダへ **`M` キー** を押すと、相手先へ移動し元の場所から削除されます。
- **削除**:
  メールを選んで `Delete` キーを押すと、サーバー上のゴミ箱へ移動します。
- **ローカルへの保存**:
  残しておきたいメールを反対画面のローカルフォルダ（例: `D:\Work\`）へ `C`（コピー）すると、通常の `.eml` ファイルとして保存されます。

### 4. 過去メール閲覧用フォルダの設定（セクション分割）

`offset` パラメータとセクション名を活用することで、同一アカウントの「通常フォルダ」と「過去メール用フォルダ」を PPc のルート階層に並べて表示できます。

```ini
; 通常フォルダ（最新100件）
[sakura]
user = user@example.sakura.ne.jp
limit = 100
offset = 0

; 過去メール用フォルダ（3,000件スキップした位置から100件）
[sakura_archive]
user = user@example.sakura.ne.jp
limit = 100
offset = 3000
```

- **PPc 上での表示（`aux://S_auximap/`）**:
  - `[D] sakura\` : 最新のメール一覧
  - `[D] sakura_archive\` : 3,000 件前のメール一覧
- **メリット**:
  - コマンドライン引数の入力やダイアログ操作を必要とせず、PPc 上でフォルダを選ぶだけで最新と過去を切り替えて閲覧・コピーが可能です。

---

## 重複メール検出・整理（report コマンド）

コマンドラインから、同一アカウント内の重複メールを検出して一覧表示、またはゴミ箱へ移動できます。

```powershell
# 特定のフォルダ（例: INBOX）内のみ重複チェック
auximap.exe report sakura INBOX

# アカウント内の全メールボックス（全フォルダ横断）で重複チェック
auximap.exe report sakura

# 検出された重複メール（2通目以降）をゴミ箱へ移動
auximap.exe report sakura INBOX --trash
auximap.exe report sakura --trash
```

#### 重複判定基準
- 送信日時（秒単位）
- 送信者（From）
- 件名（Subject）
- メールサイズ（Bytes）

上記4点がすべて一致するメールを同一と判定し、各グループの1通目を残して2通目以降を重複として報告（またはゴミ箱へ移動）します。

#### 取得件数とフォルダ横断の仕様
- **取得件数の適用単位**: `limit`（`auximap.ini` または `--limit`）は **各フォルダごと** に適用されます。全フォルダ合計ではなく、対象の各メールボックスからそれぞれ最新の最大 N 件を取得してチェックします（例: `limit = 3000` で3つのフォルダを走査する場合、各フォルダから最大3,000件、合計で最大9,000件のヘッダを走査します）。
- **重複時の保持ルール**: 重複が検出されたグループのうち、最初に走査された 1 通（同一フォルダ内なら古いメール、フォルダ横断なら先に走査されたフォルダのメール）が保持され、2 通目以降が報告・ゴミ箱移動の対象となります。
- **特定フォルダの指定**: アカウント全体ではなく単一フォルダ内のみで重複チェックを行いたい場合は、`auximap report <account> <folder>` のようにフォルダ名を指定します。

---

## 【おまけ】Emacs での使い方（auximap.el - サンプル）

本ツールにはサンプルとして、Emacs 上でメール一覧や本文をプレビューできる Elisp スクリプト `auximap.el` を同梱しています。

導入手順、設定例、キー操作などの詳細は [README_emacs.md](omake/emacs/README_emacs.md) を参照してください。

---

## 本文表示フォーマット・テンプレート設定

メール本文の表示は、テンプレートファイル `view_template.txt` で自在にカスタマイズできます。

```text
From   : {from}
To     : {to}
Cc     : {cc}
Date   : {date}
Subject: {subject}
Attach : {attachments}
--------------------------------------------------------------------------------

{body}
```

#### プレースホルダ一覧
- `{subject}` : 件名（MIMEデコード済み）
- `{from}` : 送信者名とメールアドレス
- `{to}` : 宛先
- `{cc}` : CC（※空の場合は行ごと自動で詰めて非表示になります）
- `{date}` : 送信日時（`timezone` 設定に準拠）
- `{attachments}` : 添付ファイル名とサイズ一覧（※添付が無い場合は行ごと自動で非表示になります）
- `{body}` : 本文（テキストデコード済み）

---

---

## ビルド方法（Building from Source）

本プロジェクトは Pure Rust 実装を採用しているため、外部 C コンパイラや追加ライブラリなしで誰でも簡単にビルドできます。

### 1. 前提環境
- [Rust](https://www.rust-lang.org/) 1.80 以上（stable toolchain, MSVC または GNU）

### 2. スクリプトによるビルド

付属のビルドスクリプトを実行することで、**「自動テスト ➔ リリース最適化ビルド ➔ バイナリ配置」** が行われます。

- **コマンドプロンプト**:
  ```cmd
  build.bat
  ```
- **PowerShell**:
  ```powershell
  .\build.ps1
  ```

### 3. 通常の cargo コマンドでビルドする場合

```powershell
# 1. ユニットテストの実行（全11項目：パース・UTF-7等の検証）
cargo test

# 2. リリースビルド（LTO + Strip 最適化）
cargo build --release
```
- ビルド成果物は `target/release/auximap.exe`（または `.cargo/config.toml` の指定先）に出力されます。
- `Cargo.toml` の `[profile.release]` により、不要なデバッグシンボルが自動削除され（`strip = true`）、リンク時最適化（`lto = true`）された単一バイナリが生成されます。

---

## 免責事項（Disclaimer）

本ツール（`auximap.exe`）は、Paper Plane xUI（PPx）の `aux:` パス機能を利用して有志が個人制作した非公式の連携ツールです。プログラムやドキュメントの大部分は AI との対話を通じて作成されています。  
一般的な IMAP 仕様に準拠するよう実装していますが、作者の手元で実際に動作確認を行っている環境は Google（Gmail）および さくらインターネットのみです。その他のメールサーバーでは正しく動作しない可能性があります。  
PPx の作者である TORO 氏の著作物ではありません。本ツールに関する質問・要望・不具合報告などを TORO 氏へ問い合わせることはご遠慮ください。

---

## ライセンス（License）

本ソフトウェアは [MIT License](LICENSE) のもとで公開されています。商用・非商用問わず自由にご利用・改変・再配布いただけます。

