# runtrol

> [!IMPORTANT]
> **最上位の製品規則: Runtrol サイドバーだけで、この PC に接続されたすべてのコーディングエージェント CLI、現在のプロジェクト、実際の会話名、実行中の会話を示す回転するエージェントアイコン、使用量をクリックせずに把握して管理できなければならない。別タブ、折りたたまれたビュー、重複ラベル、誤った階層の背後に情報が隠れる場合はリリースできない。**

**一つの VS Code ウィンドウですべてのプロジェクト、セッション、エージェントを即座に運用する。**

[한국어](README.md) | [English](README_EN.md) | [中文](README_ZH.md) | 日本語

> ステータス: **コアと主力 VS Code 拡張を実装し、`Runtrol Studio 0.1.21` を六つの native target 向けに公開した。** live session index、実物 Extension Host と秒間 3,000 frame Webview の性能 ratchet、インストール済み実物 CLI の完全な操作 journey、Marketplace からの clean install、active session を維持する VSIX upgrade と rollback を検証済みである。独立したデスクトップ GUI のコードと実行経路は削除され、VS Code 拡張が唯一の PC surface である。公開 Runtime protocol、Rust と TypeScript SDK、外部 packed consumer gate、署名付き六 target standalone Runtime release pipeline も実装した。確認済み provider channel の自動更新には process exclusion と正確な rollback も実装した。[Marketplace 拡張](https://marketplace.visualstudio.com/items?itemName=runtrol.runtrol-studio)、[GitHub Pages サイト](https://eddmpython.github.io/runtrol/)、リレー接続のスマートフォン PWA、本文を持たない Web Push、Mission 監督 surface を実装した。会話ヘッダーのチップから応答モデルと permission mode を会話中に切り替えられ、選択はその CLI 自身の切り替え surface に中継され、チップはサービスが答えた値だけを表示する(インストール済み実物 Claude Code の journey gate が切り替えと復帰を CLI 自身のアナウンスで検証する)。現在の VS Code フォルダーは登録なしで最初のプロジェクト見出しとなり、その他のフォルダーは繰り返し使われた場合だけプロジェクトとしてまとまる。各会話行にはエージェントアイコンと実際の会話名だけを表示し、実行中はそのアイコン自体が回転する。ペアリング済みデバイスの権限は承認された workspace root に限定される: session index、すべての session コマンド、Mission の読み取りが同じ live root 検証を通り、root の取り消しは即時に効く。provider の準備は provider ごとの lane で並列化され、cold の初回 5 件が直列 18.1 秒に対して 8.7 秒で終わり、新しく開いたフォルダーの既存会話はリフレッシュなしで届く。会話はそれぞれ独立したエディタータブで開き、複数の会話を同時に並べて画面分割でき、入力と中断はそのタブの会話に向かう。拡張の更新後も動き続けていた旧 Core は、マシンがアイドルになった瞬間に新しいビルドへ自ら入れ替わり、更新が再起動なしで実際に届く。active gate は shipped PWA module を production daemon とインストール済み実物 CLI に通し、session、approval、remote disconnect recovery journey を検証する。プロジェクト見出しを一度クリックすると Agent Tools が有効になり、インストール済みコーディングエージェントは project root に限定された七つの公開 Runtime tool で作業を委譲できる。無効化すると provider 登録、Runtime 権限、保護されたローカル資格情報が削除される。iOS 実機へのインストールと Web Push の運用確認は未検証の contributor operator evidence として残し、現在の完了範囲とスコアから除外する。以下のスコアの多くが 0 なのは、コードがないからではなく、その軸を断言するゲートがまだない
> からである。
> Fleet Compare は同じレビュー済み指示を 2 から 4 個の隔離 worktree と provider session に一括送信し、VS Code の grid と native diff で比較した後、選択した一つの passing Receipt だけで最終検証する。この流れは実物二 CLI gate と実際の Extension Host 目視検査で検証した。
> 通常の Mission は `Continue Reviewed Mission` 一回で現在安全な wave を開始し、完了した Task を固定 Gate で封印し、次の DAG wave を準備して正確なレビュー済み指示を送る。実物 CLI と Extension Host の二段階 journey で検証した。
> `Continue Ready Missions` は最大八つの正確な Mission digest を一度にレビューし、複数 project の現在安全な wave をまとめて進める。実物 Extension Host で二つの Git project を一回で開始し、次の一回で両方を `integrating` に進めた。
>
> `Review and Apply Mission Landing` は通常 Mission のすべての passing Receipt Artifact を、現在の project と
> 一つの VS Code native multi-diff で比較する。`Apply, run Gates and complete` 一回で Mission、Receipt、source と
> target の byte、link、未保存 editor を再確認し、既存 file と新規 file にレビュー済みの正確な byte を適用して
> Core の固定 Gate を実行する。実物 Extension Host で二つの Git project の四 Artifact を適用し、project と Receipt
> の drift を拒否して復旧し、最初の完了中も二つ目を待機させたまま次の Landing を開いた。
>
> Fleet Compare は比較だけで止まらない。一つの passing Task を選ぶと、その Task と Receipt だけを含む native
> winner multi-diff が開く。公開 apply 操作一回で他の候補を混ぜずに正確な byte を書き、固定 Gate を実行して
> Core で Mission を完了する。Core は選択した Task と Receipt を永続的な終端証拠として保持するため、応答消失
> 後の復旧が別の候補を成功と誤認しない。実物 Extension Host journey は異なる二つの実物 CLI 結果から
> `attempt-2` だけを適用して `completed` に到達し、各目標画面を直接目視検査した。
>
> `Mission Auto Flight` はレビュー済みの通常 Mission を PC で一度 arm し、各実物 provider turn を
> lifecycle generation で証明して固定 Gate と Receipt を封印し、次の安全な DAG wave を自動で開始する。
> person、quota 待ちと pause はそのまま待ち、権限 drift、曖昧な送信、recovery state では即座に解除する。
> 実物二 wave CLI journey はオペレーターの Continue 0 回で `integrating` 到着と自動権限回収を検証し、
> 三つの画面状態を直接目視確認した。最終 Receipt Landing と integration は常に明示的な操作として残る。
>
> スマートフォン通知は会話内容を運ばず、実際にオペレーターを待つ最初の session を開く。`Needs you`
> の件数と次への移動は person wait だけを含み、account limit は区別する。実物 CLI approval gate が
> approval 中の表示と回答後の解除を検証する。
>
> Auto Flight の person wait、安全な停止、Receipt Landing も同じ内容なしの通知を使う。認証後、
> スマートフォンは Core の最大 64 件の構造 signal を読み、現在の root、Mission digest、state が一致する
> 正確な session または Mission だけを開く。push に Mission ID、instruction、path、output はなく、端末には
> opaque cursor だけが残る。

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
- **マルチセッションの費用はセッション数に比例しない。** 15 セッションを日常運用の基準、30 セッションを release gate の負荷とする。論理セッションはさらに存在できるが、hot process は最大 8 個、active renderer と full stream は正確に一つである。選択セッションの固定、即時検索、安定した並び順、workspace 切り替えは 30 セッションでも同じ操作で行う。
- **マルチエージェントは provider-neutral である。** 対応するインストール済み CLI を自動検出し、一つの一覧と同じ操作法で運用する。新しい provider は core を変更せず manifest または driver で追加する。
- **エージェントがリポジトリを自律的に変更する。** provider CLI が作業と会話を所有し、runtrol は session、workspace、worktree、process lifecycle、collision boundary だけを監督する。
- **会話選択と workspace 切り替えを結び付ける。** session 選択時に会話とファイル文脈を即座に切り替え、実際の編集が必要な時だけ正確な workspace または worktree を Code-hot にする。会話本文から path を推測しない。
- **デバイス接続とセッション所有権を分離する。** VS Code とスマートフォンは同じ Core にペアリングされた操作面であり、どちらもセッションを所有しない。ウィンドウ、デバイス、ネットワーク経路が変わっても Core がセッションを維持する。Tailscale など既存のプライベートネットワークは検出時に直結経路として使えるが、ペアリング、push、正しさはそれに依存しない。
- **人間が常に最優先である。** 長い streaming、複数 agent、build、test の最中でも入力、スクロール、editor、ファイル移動が先に反応する。
- **薄い境界は変わらない。** provider account credential、transcript、model API key、conversation copy を所有しない。

現在の合計は **74/140、平均 5.3/10** である。有効な CI ゲートが立つ軸は十三である。
10 点は、実際の環境で完結した道筋が繰り返し検証された状態を指す。
**manual 層を超えるスコアの根拠は CI で実際に動くゲートである。自動実行されない経路は、どれほど実装済みに見えても 3 点を超えない。**

| 北極星 | 現在のスコア | 現状 | 到達すべき状態 |
|---|---:|---|---|
| 一つのセッション一覧 | 5/10 | hosted CI が実物 VS Code Extension Host で開始、二つの workspace 切り替え、正確な選択復元、再接続、中断、終了を検証する。相手は決定論的な loopback model なので mock 層に留まる。 | プロバイダーが Claude Code でも Codex でも、その次の何かでも、いま自分の PC で生きているセッションが一つの一覧に並び、開始・再開・削除がそこで完結する。 |
| 即座の反応 | 5/10 | 実物 VS Code Extension Host が production bundle を測る。一つの ratchet が実物 30 セッション一覧、最大 8 個の hot ACP process、provider-native cold resume、毎秒 3,000 個の raw frame、Core watch の確認と Webview paint まで終えた session 切り替え、workspace 変更後の正確な選択復元を覆う。transport の相手は mock なので、この tier に留まる。 | 一覧が待ち時間なく現れ、会話は押した瞬間に開き、長い出力が流れてもスクロールと入力が途切れない。ユーザーが読み込みを意識する瞬間が存在しない。 |
| スマホから自分の PC のセッションへ | 5/10 | hosted CI は shipped PWA の WebCrypto、Noise、CoreClient module を headless phone process で実行し、production daemon を通じてインストール済みの実物 Claude Code session を開始し、prompt、watch output、終了を往復する。model counterpart は決定論的 loopback fixture なので mock 層である。 | スマートフォンを PC に一度つないでおけば、席を離れた後もその PC で動いているセッションに新しい指示を入れ、出力をリアルタイムで見られる。プロバイダーアカウントのプランや認証方式がこの体験を妨げない。 |
| プロバイダー拡張性 | 5/10 | hosted CI は外部ドライバーの公開契約、三つの OS 上の汎用 ACP fixture、独立配布 ACP 実装による二つの turn と native load、実物 Claude Code の hidden approval 拒否往復を検証する。model endpoint はローカル mock である。scheduled CI は最新 CLI で parser probe と同じ approval journey を繰り返すが、アカウント model の動作や event 全表面は主張しない。 | 新しい CLI が出たらアダプターを一つ足すだけで、PC 画面もスマホ画面も操作方法もそのまま。ユーザーはプロバイダーが増えたことを一覧が長くなったこととしてだけ知る。 |
| 会話を通さない | 6/10 | `egressContract` は実物の loopback socket で正確な送信 allowlist と production Noise IK、IKpsk1 境界を動かす。prompt の標本は relay capture や診断文字列に平文で現れず、transport は disk と log の API を持たず、driver と storage は provider の transcript path を知らない。実物のスマートフォンと relay を結ぶ live gate がないため、天井は 6 である。 | ユーザーのプロンプトとモデルの応答は、PC とプロバイダーの間、そしてユーザー自身のデバイスの間だけを往復する。runtrol はその本文を保存せず、途中のどのサーバーも読める形でそれを受け取らない。 |
| スマホで承認 | 5/10 | active gate が実物 Claude Code の hidden Write approval を PWA watch path で受け、完全な subject、唯一の `rejectOnce`、32-byte digest を確認して拒否し、同じ provider turn の再開と終了を検証する。model counterpart は決定論的 loopback fixture なので mock 層である。 | エージェントが危険な操作の前で止まるとスマートフォンに表示され、そこで許可または拒否すると PC のセッションがただちに続く。 |
| 切れても生き残る | 5/10 | shipped PWA module とインストール済み実物 CLI が network cut 後に exact cursor から replay し、Core restart 後は明示的 gap と native resume で続行する。model counterpart は mock である。 | スマートフォンのロック、ネットワーク切断、runtrol 再起動の後も、PC セッションは公式 resume surface から復旧できる。保持範囲内は正確な cursor から続き、範囲外は黙って飛ばさず明示的な gap になる。 |
| 常駐コスト | 6/10 | 三つの hosted OS が一つの ratchet で実物 debug daemon の idle RSS と 10 秒間の idle CPU を測る。独立した二種類目の証拠がないため、上限は 6 のままである。 | 一日中つけっぱなしでも、ユーザーはその存在に気づかない。バッテリーにも、ファンにも、タスクマネージャーにも見えない。 |
| どこでも同じやり方 | 8/10 | 有効な hosted CI が正確な native VSIX をクリーンな VS Code にインストールし、手動の Core path なしで同梱 Core を発見して Runtrol を開き、公開 command から `New chat` composer tab を開いて閉じる。同じ gate が Windows、macOS、Linux で動き、release matrix は x64 と ARM64 の六つの target でも繰り返す。主張するのは static contract、実物 journey、multi-OS evidence に限る。 | Windows、macOS、Linux でインストール方法も操作も同じ。Windows ユーザーが WSL や tmux を知る必要がない。 |
| 勝手に最新 | 5/10 | `vscodeUpgradeRollback` は三つの OS で VSIX と Core 置換中の session continuity を検証する。`cliUpdateRehearsal` は決定的 fixture により、確認済み provider 更新の失敗、正確な復元、振動防止を検証する。hosted CI は実アカウントの provider installation を変更しないため、証拠は mock 層のままである。 | アプリとインストール済みのエージェント CLI が自動で最新を保ち、更新がセッションを壊したらユーザーが手を触れる前に戻っている。ユーザーがバージョンを気にする瞬間が存在しない。 |
| モデル自動認識 | 6/10 | hosted `modelDetectionSmoke --require-all` は資格情報なしで最新の実物 CLI を導入し、Codex の `model/list` と隔離した provider-owned option cache sentinel を含む Claude partial catalogue を検査し、観測した identifier が production source にハードコードされていないことを確認する。特定アカウントでの利用可否までは証明しないため、live gate 一種類の上限 6 である。 | いまこのアカウントで実際に使えるモデルがそのまま一覧に出て、新しいモデルが出ても runtrol を直さずに現れる。 |
| セッション同士が踏まない | 5/10 | 実際の Git metadata と production Core admission が同じ worktree の下位フォルダを一つの writer として扱い、opening、live、closing の重複予約を原子的に拒否する。linked worktree と運用者が明示した共有開始は区別する。provider は fixture なので mock 層である。 | どのセッションがどのフォルダで何を変えているかが常に区別でき、二つ目のセッションが同じフォルダに触れそうなときは開始前に警告され、プロバイダーが隔離手段（ワークツリー）を出しているなら開始画面でそのまま使える。 |
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
| **PC（Windows、macOS、Linux）** | [VS Code Marketplace から `Runtrol Studio`](https://marketplace.visualstudio.com/items?itemName=runtrol.runtrol-studio) をインストールする。x64 と ARM64 に対応し、独立したデスクトップアプリは配布しない |
| **モバイル** | [恒久的な GitHub Pages オリジンのスマートフォン PWA](https://eddmpython.github.io/runtrol/app/)。まず VS Code の一回限りの QR でペアリングする |

公開リリース `0.1.21` と六つの platform VSIX は [GitHub Releases](https://github.com/eddmpython/runtrol/releases/tag/vscode-v0.1.21) からも取得できる。
Marketplace からインストールした拡張は VS Code が自動更新する。以前の版を VSIX から直接インストールした場合、VS Code はその拡張の自動更新を無効にするため、Marketplace から一度再インストールする。

## エージェントに Runtrol を使わせる

プロジェクト見出しの sparkle を選び、**Enable Agent Tools for This Project** を実行する。インストール済み
コーディングエージェントは provider と model を発見し、project session の開始、instruction の送信、
event の読み取り、正確な session の停止を行える。プロジェクト行に `Agent Tools` と表示されれば準備完了である。

権限はその canonical project root 一つに限定される。approval の応答、会話の削除、暗黙の shared start、
API key、transcript の複製、Runtrol 所有 agent loop は存在しない。**Disable Agent Tools for This Project**
は Runtime 権限と OS 保護資格情報を削除し、最後の project を無効化した場合は provider 登録も削除する。
正確な契約は [Agent Tools](docs/agentTools.md) にある。

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
- **独自のエージェントフレームワークではない。** Runtrol は planner や autonomous loop を所有しない。provider 所有 agent loop に bounded Runtime tool を提供するが、その loop 自体にはならない。
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
| `crates/` | 製品コア（Rust）。デーモン、プロバイダーアダプター、トランスポート。独立 GUI crate は存在しない | 実装済み |
| [`clients/typescript/`](clients/typescript/) | 外部製品向け公開 Runtime TypeScript SDK | packed consumer 検証済み |
| [`extensions/runtrol-vscode/`](extensions/runtrol-vscode/) | 唯一の PC surface `Runtrol Studio` | 30 session のリリース負荷を検証、0.1.21 公開済み |
| [`pwa/`](pwa/) | モバイル PWA | リレー接続、セッション制御、承認、`Needs you` と Mission Flight Signals の正確な focus を実装済み |
| [`site/`](site/) | [依存関係のない GitHub Pages ランディング](https://eddmpython.github.io/runtrol/) | 公開済み |
| [`assets/brand/`](assets/brand/) | ロゴ。SVG が正本で、favicon・アイコン・ソーシャルカードはそこから派生する | |
| [`docs/`](docs/README.md) | 運用ドキュメントの正本 | |
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

製品本体は [AGPL-3.0-only](LICENSE)。公開クライアントパッケージ (`runtrol-runtime-protocol` ·
`runtrol-runtime-client` · `@runtrol/runtime-client`) は他のプログラムがリンクするためのものなので
[Apache-2.0](crates/runtrol-runtime-protocol/LICENSE)。

runtrol を使うだけでは、あなたのコードに何の義務も生じません。runtrol はエージェント CLI を
別プロセスとして監督するだけで、あなたが書いたものにリンクされません。
