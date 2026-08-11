# runtrol

**一つの VS Code ウィンドウですべてのプロジェクト、セッション、エージェントを即座に運用する。**

[한국어](README.md) | [English](README_EN.md) | [中文](README_ZH.md) | 日本語

> ステータス: **コア、Windows デスクトップ、VS Code 拡張の最初の end-to-end slice、live session index、実物 Extension Host と秒間 3,000 frame Webview の性能 ratchet、platform VSIX 内容ゲート、Windows のクリーンインストールを実装済み。** `Runtrol Studio` は Core 自動検出、インストール済み CLI 一覧、マルチセッション TreeView、変更時だけ届くセッション snapshot、再利用する command channel、選択セッション一つの購読、bounded live view、同一ウィンドウでの workspace 切り替えを提供する。hosted マルチプラットフォーム package 検証、Marketplace 署名と公開はまだ残っている。以下のスコアの多くが 0 なのは、コードがないからではなく、その軸を断言するゲートがまだない
> からである。

The security boundary and default-deny settings are documented in [SECURITY.md](SECURITY.md).

## 北極星

**runtrol は一つの VS Code ウィンドウを、すべてのプロジェクト、対応するインストール済み
コーディングエージェント CLI、provider 所有セッションの control plane にする。各エージェントは
結び付けられたリポジトリを自律的に変更する。runtrol はセッションを生かし、同時作業を隔離し、
会話本文を解釈せずに選択したセッションを正確な workspace または worktree に接続する。
セッションとエージェントが増えても renderer、active subscription、Code-hot workspace は bounded のまま。
streaming と background 作業が入力、スクロール、セッション切り替え、ファイル移動を詰まらせてはならない。
インストール済み CLI、モデル、capability は runtime に自動検出する。会話はユーザーの PC と provider の間だけを往復し、runtrol はその間に割り込まない。**

### 変わらない中核

- **機能と速度は一つの契約である。** 機能が増えても待ち時間や引っかかりを許さない。目に見える遅延、frame drop、入力遅延は release を止めるバグである。
- **マルチセッションの費用はセッション数に比例しない。** 論理セッションは多数存在できるが、active renderer と full stream は正確に一つである。
- **マルチエージェントは provider-neutral である。** 対応するインストール済み CLI を自動検出し、一つの一覧と同じ操作法で運用する。新しい provider は core を変更せず manifest または driver で追加する。
- **エージェントがリポジトリを自律的に変更する。** provider CLI が作業と会話を所有し、runtrol は session、workspace、worktree、process lifecycle、collision boundary だけを監督する。
- **会話選択と workspace 切り替えを結び付ける。** session 選択時に会話とファイル文脈を即座に切り替え、実際の編集が必要な時だけ正確な workspace または worktree を Code-hot にする。会話本文から path を推測しない。
- **デバイス接続とセッション所有権を分離する。** VS Code とスマートフォンは同じ Core にペアリングされた操作面であり、どちらもセッションを所有しない。ウィンドウ、デバイス、ネットワーク経路が変わっても Core がセッションを維持する。
- **人間が常に最優先である。** 長い streaming、複数 agent、build、test の最中でも入力、スクロール、editor、ファイル移動が先に反応する。
- **薄い境界は変わらない。** credential、transcript、model API key、conversation copy を所有しない。

現在の合計は **41/140、平均 2.9/10** である。有効な CI ゲートが立つ軸は七つである。
10 点は、実際の環境で完結した道筋が繰り返し検証された状態を指す。
**manual 層を超えるスコアの根拠は CI で実際に動くゲートである。自動実行されない経路は、どれほど実装済みに見えても 3 点を超えない。**

| 北極星 | 現在のスコア | 現状 | 到達すべき状態 |
|---|---:|---|---|
| 一つのセッション一覧 | 5/10 | hosted Windows CI が production browser lifecycle と実際の Tauri 製品を動かし、開始、hot および cold session の open、編集可能な次の入力、確認済み一覧削除を検証する。相手は決定論的な mock transport と ACP fixture なので mock 層に留まる。 | プロバイダーが Claude Code でも Codex でも、その次の何かでも、いま自分の PC で生きているセッションが一つの一覧に並び、開始・再開・削除がそこで完結する。 |
| 即座の反応 | 5/10 | 実ブラウザで production bundle を測り、一覧、会話を開く操作、入力遅延の ratchet を守りながら毎秒 3,000 個の raw frame を処理する。transport の相手は mock なので、この tier に留まる。 | 一覧が待ち時間なく現れ、会話は押した瞬間に開き、長い出力が流れてもスクロールと入力が途切れない。ユーザーが読み込みを意識する瞬間が存在しない。 |
| スマホから自分の PC のセッションへ | 0/10 | 未実装。 | スマートフォンを PC に一度つないでおけば、席を離れた後もその PC で動いているセッションに新しい指示を入れ、出力をリアルタイムで見られる。プロバイダーアカウントのプランや認証方式がこの体験を妨げない。 |
| プロバイダー拡張性 | 5/10 | hosted CI は外部ドライバーの公開契約、三つの OS 上の汎用 ACP fixture、独立配布 ACP 実装による二つの turn と native load、実物 Claude Code の hidden approval 拒否往復を検証する。model endpoint はローカル mock である。scheduled CI は最新 CLI で parser probe と同じ approval journey を繰り返すが、アカウント model の動作や event 全表面は主張しない。 | 新しい CLI が出たらアダプターを一つ足すだけで、PC 画面もスマホ画面も操作方法もそのまま。ユーザーはプロバイダーが増えたことを一覧が長くなったこととしてだけ知る。 |
| 会話を通さない | 6/10 | `egressContract` は実物の loopback socket で正確な送信 allowlist と production Noise IK、IKpsk1 境界を動かす。prompt の標本は relay capture や診断文字列に平文で現れず、transport は disk と log の API を持たず、driver と storage は provider の transcript path を知らない。実物のスマートフォンと relay を結ぶ live gate がないため、天井は 6 である。 | ユーザーのプロンプトとモデルの応答は、PC とプロバイダーの間、そしてユーザー自身のデバイスの間だけを往復する。runtrol はその本文を保存せず、途中のどのサーバーも読める形でそれを受け取らない。 |
| スマホで承認 | 0/10 | 未実装。 | エージェントが危険な操作の前で止まるとスマートフォンに表示され、そこで許可または拒否すると PC のセッションがただちに続く。 |
| 切れても生き残る | 0/10 | リモートのスマートフォンを含む end-to-end ゲートは未実装。 | スマートフォンのロック、ネットワーク切断、runtrol 再起動の後も、PC セッションは公式 resume surface から復旧できる。保持範囲内は正確な cursor から続き、範囲外は黙って飛ばさず明示的な gap になる。 |
| 常駐コスト | 6/10 | 三つの hosted OS が実物 debug daemon の idle RSS と CPU を測る。Windows は production GUI と WebView2 process tree 全体を 60 秒 ratchet にも照合する。どちらも bench 証拠なので上限は 6 のままで、24 時間 campaign は別契約である。 | 一日中つけっぱなしでも、ユーザーはその存在に気づかない。バッテリーにも、ファンにも、タスクマネージャーにも見えない。 |
| どこでも同じやり方 | 0/10 | 未実装。 | Windows、macOS、Linux でインストール方法も操作も同じ。Windows ユーザーが WSL や tmux を知る必要がない。 |
| 勝手に最新 | 0/10 | 未実装。 | アプリとインストール済みのエージェント CLI が自動で最新を保ち、更新がセッションを壊したらユーザーが手を触れる前に戻っている。ユーザーがバージョンを気にする瞬間が存在しない。 |
| モデル自動認識 | 6/10 | hosted `modelDetectionSmoke --require-all` は資格情報なしで最新の実物 CLI を導入し、Codex の `model/list` と隔離した provider-owned option cache sentinel を含む Claude partial catalogue を検査し、観測した identifier が production source にハードコードされていないことを確認する。特定アカウントでの利用可否までは証明しないため、live gate 一種類の上限 6 である。 | いまこのアカウントで実際に使えるモデルがそのまま一覧に出て、新しいモデルが出ても runtrol を直さずに現れる。 |
| セッション同士が踏まない | 0/10 | 未実装。 | どのセッションがどのフォルダで何を変えているかが常に区別でき、二つ目のセッションが同じフォルダに触れそうなときは開始前に警告され、プロバイダーが隔離手段（ワークツリー）を出しているなら開始画面でそのまま使える。 |
| AI 同士が相談し合う | 3/10 | トグルが実物の二つの CLI を各自の公式コマンドで配線・検証・復元し、実際のターン中の相談受信まで手動で実測した（2026-08-03）。`crossConsultSmoke` は実物のサブスクリプション CLI を動かすため運用者のマシンで走り、hosted CI ゲートがないので manual 層。 | トグル一つで二つの CLI が互いを公式表面（MCP）で登録し、一方の AI がターン中にもう一方の意見を直接受け取る。配線は各 CLI 自身の公式コマンドだけで作り（設定ファイルを直接書かない）、会話本文は依然として runtrol を通らない。ユーザーは MCP という概念を知らなくていい。 |
| 去る自由 | 5/10 | `uninstallLeavesNoTrace` は runtrol home の外にプロバイダー状態を置いて一つのターンを終え、home 全体を削除した後、新しい daemon で同じ native session を読み込み二つ目のターンを終える。相手は ACP fixture なので mock 層である。 | runtrol を消してもセッションと記録は各 CLI のものとしてそのまま残り、元のやり方で続けられる。runtrol が人質に取るデータがない。 |

どのゲートがどの軸を支えるかは [docs/northStarEvidence.md](docs/northStarEvidence.md) が正本である。

### 採点基準

スコアは人が選ぶ等級ではなく、**基盤層 + 加算**という計算で決まる。正本は
[tests/audit/northStar/board.toml](tests/audit/northStar/board.toml) であり、`northStarBoard`
ゲートが計算し、`readmeParity` ゲートが 4 言語の README をその計算結果と突き合わせる。

**基盤層。** 軸ごとに一つだけ成立し、これが天井である。

| 基盤層 | スコア | 成立条件 |
|---|---:|---|
| `none` | 0 | この軸を断言するゲートがない |
| `manual` | 3 | 人が手で一度動くのを見た。有効な hosted CI ゲートがない。デモ動画、スクリーンショット、「動かしたら動いた」はすべてここ |
| `mock` | 5 | 登録されたゲートは動くが相手が偽物。mock CLI、stub プロバイダー、シミュレートされたスマートフォン |
| `realOneKind` | 6 | 実物相手に動く。ただし static (`contract`) と live (`smoke`、`bench`) のうち一種類しかない |
| `realBothKinds` | 7 | 実物相手に static と live の両方を備えている |

**加算。** `realBothKinds` でのみ付く。各加算はそれに合う種類のゲートを要求し、四つ揃えたときちょうど 10 になる。

| 加算 | スコア | 成立条件 |
|---|---:|---|
| `multiProvider` | +1 | 同じゲートがプロバイダー二つ以上で green |
| `multiOs` | +1 | 同じゲートが OS 二つ以上で green。Windows を含む |
| `faultInjection` | +0.5 | 障害注入（デーモン強制終了、ネットワーク遮断）を通しても green |
| `ratchet` | +0.5 | 回帰 ratchet があり、数値が悪化した瞬間に red になる |

スコアが水増しされないための規則:

1. **実装がどれほど完成して見えても、ランナーが実際に呼ぶゲートがなければ上限は `manual`（3 点）である。** 例外なし。
2. `operator` 種のゲート（実機や実アカウントを要するもの）は**合計から外す**。
3. スコアを上げる PR には**ゲート名と CI 実行へのリンク**を本文に添え、同じコミットで `board.toml` を直す。散文はスコアではない。
4. スコアは 0.5 刻みでのみ付ける。8.7 のような数字は精密さではなく自己欺瞞である。
5. **ある軸が下がることを妨げない。** プロバイダーが表面を変えてゲートが red になればスコアは下がる。この表は昨日の自慢ではなく今日の状態である。
6. **天井を決めるのは実行回数ではなく、欠けているゲートの種類である。** 一種類しか持たない軸はどれだけ green でも 6 を超えられない。現在 14 軸のうち 13 軸がその状態で、`northStarBoard` が軸ごとにその天井を印字する。

### スコアになるものとならないもの

三つの層は混ぜない。混ぜた瞬間、ユーザーが何も受け取っていないのに合計が上がる。

| 層 | 何が入るか | どう表示されるか |
|---|---|---|
| **スコア軸** | ユーザーが体感する結果（上の表の 14 個） | 0 から 10、合計 /140 |
| **床ゲート** | モジュール化、クリーンコード、セキュリティ、衛生、予算 | **スコアではない。** green か red だけで、red はマージされない |
| **撤退条件** | 革新性、ポジショニング | **数字がない。** [docs/positioning.md](docs/positioning.md) の kill criteria だけが判定する |

- **モジュール化とクリーンコードに部分点を与えない理由。** どちらも強行規則である。「クリーンコード 7/10」は「3 だけ規則を破っている」という意味であり、それはスコアではなく red である。代わりに項目ごとのゲートに分けて名前を付ける（`dependencyDirection`、`providerIsolation`、`checkSilentFail`、`cargoClippy` など）。全一覧は [docs/northStarEvidence.md](docs/northStarEvidence.md) にある。
- **革新性に数字を与えない理由。** 革新とは上の 14 軸そのものである（「複数の AI を一か所で管理する」）。別に点を付ければ同じものを二度数えることになり、どのゲートもその数字を断言できないので規則 3 に触れる。革新が消えたかどうかは kill criteria が判定する。

## 最上位の原則: ユーザーの利便性

どの分かれ道でもユーザーがより楽な側を選ぶ。判定基準は好みではなく、**ユーザーが実際に行う操作の数と待つ時間**である。

- 本来自動で済むはずのことをユーザーが設定しなければならないなら、失敗である
- ユーザーが自分の待ち時間を認識できるなら、失敗である
- ユーザーが概念を学ばなければならないなら（tmux、WSL、トンネル、ポートフォワーディング、証明書のインストール）、失敗である
- ユーザーが同じことを二度するなら、失敗である
- **カクつきは最適化の対象ではなく不具合である**

## 入手

| | |
|---|---|
| **PC（Windows）** | まだ未リリース。`Runtrol Studio` 拡張ソースと Core 直接 IPC は実装済みである。主力配布目標は検証済み bundled Core を含む Marketplace 拡張である |
| **PC（macOS、Linux）** | 準備中 |
| **モバイル** | PWA。ブラウザで開いてホーム画面に追加する。アプリストアは不要 |

まだリリースはない。コアと Windows デスクトップは実装済みで、配布面は準備中である。

## runtrol が要らない人

**プロバイダーを一つしか使わないなら、そのプロバイダー自身のリモートコントロールの方が良い。これを先に書いておく。**

Claude Code だけを使う人には `claude --remote-control` の方が良い。作った当人が作り、
無料で同梱され、ネイティブプッシュが付き、アプリストアにある。
Anthropic、OpenAI、GitHub、Amp の四社ともすでに自社のリモートコントロールを出している。それで足りるならそれを使えばよい。

**runtrol はその一覧が四つに割れている人のためのものである。**
Claude アプリに Codex のセッションは永遠に出ない。これは機能差ではなく構造であり、プロバイダーに直す理由がない。

## runtrol でないもの

- **チャットクライアントではない。** 会話の描画は各 CLI がすでにやっていることである。runtrol はその出力を運ぶだけで解釈しない。
- **モデルプロキシではない。** モデル API を呼ばず、トークンを読まず、リクエストを中継しない。設計上の好みではなく生存条件である。
- **IDE ではない。** diff を見せるところまでが境界で、編集することは境界の外である。
- **エージェントフレームワークではない。** プランナー、サブエージェントのオーケストレーション、自律ループはない。それは各 CLI の中にすでにあり、そちらの方が上手い。
- **ホスティングサービスではない。** アカウントもログインも料金プランもない。
- **ターミナルマルチプレクサではない。** tmux を置き換えるのではなく、**要求しない**ためのものである。

## なぜ Rust なのか

正直に言えば、**Rust 自体は差別化要因ではない。** この分野の競合は十以上がすでに Rust である。
Rust は目的ではなく、上の表の三つの軸のための手段である。

- **どこでも同じやり方**: ConPTY と POSIX を同じ抽象の後ろで直接扱い、tmux なしで Windows を一級にする。
- **常駐コスト**: これは一日中つけっぱなしのデーモンである。ランタイムのない単一静的バイナリなら Node も Python も入れる必要がない。
- **即座の反応**: 一覧と会話が待ち時間なく開くには、GC 停止とランタイム起動がないことが要る。

その軸をゲートで固定しなければ、Rust を使う意味は消える。

## 構成

| | | |
|---|---|---|
| `crates/` | 製品（Rust）。デーモン、プロバイダーアダプター、トランスポート、デスクトップアプリ | 実装済み |
| [`extensions/runtrol-vscode/`](extensions/runtrol-vscode/) | VS Code の主力 surface `Runtrol Studio` | 最初の垂直 slice を実装、未リリース |
| `pwa/` | モバイル PWA | 未作成 |
| `site/` | GitHub Pages ランディング | 未作成 |
| [`assets/brand/`](assets/brand/) | ロゴ。SVG が正本で、favicon・アイコン・ソーシャルカードはそこから派生する | |
| [`docs/`](docs/README.md) | 運用ドキュメントの正本 | |
| [`mainPlan/`](mainPlan/README.md) | これから作るもの（イニシアチブ。完了したら知識を `docs/` へ昇格しフォルダを削除する） | |
| [`tests/audit/`](tests/audit/) | 契約ゲート | |
| [`tests/audit/northStar/`](tests/audit/northStar/) | スコアボードのエンジン。上の表の数字を計算し、4 言語をそれに揃える | |

## 開発

```bash
python -X utf8 tests/audit/preflight.py          # ローカル CI 全体
python -X utf8 tests/audit/preflight.py lint     # lint のみ
python -X utf8 tests/audit/preflight.py --list   # 何が動き何がスキップされるか
git config core.hooksPath .githooks              # クローンごとに一度
```

ゲートは**通過の判子ではなく欠陥の検出器**である。新しいゲートを立てたら、通るのを見る前に、
捕まえるべき欠陥をわざと仕込んで red になるかを確かめる
（`python -X utf8 tests/audit/checkSilentFail.py --selftest` がその形である）。

貢献は [CONTRIBUTING.md](CONTRIBUTING.md) を見る。設計段階の貢献も本物の貢献である。

## ライセンス

[MIT](LICENSE)
