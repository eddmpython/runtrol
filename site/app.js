import { inferNativeTarget, selectTargetVsix } from "./release-assets.mjs";

const PROJECT = Object.freeze({
  repository: "https://github.com/eddmpython/runtrol",
  releasesApi: "https://api.github.com/repos/eddmpython/runtrol/releases/latest",
});

const COPY = {
  en: {
    navProduct: "Product",
    navInstall: "Install",
    navPhone: "Phone",
    heroEyebrow: "OPEN SOURCE CONTROL PLANE",
    heroTitle: "One VS Code window.<br>Every session in reach.",
    heroLede: "runtrol supervises the coding-agent CLIs already installed on your machine. Switch repositories, resume provider-owned sessions, and keep 30 conversations organized without turning VS Code into a slow chat archive.",
    installMarketplace: "Install from Marketplace",
    viewSource: "View source",
    releaseChecking: "Checking GitHub Releases for a VSIX.",
    releaseMissing: "No public VSIX has been released yet.",
    releaseFound: "A manual VSIX is available from GitHub Releases.",
    sessions: "Sessions",
    selectedSession: "SELECTED SESSION",
    workspaceFollows: "WORKSPACE FOLLOWS SELECTION",
    proofSessions: "session release gate",
    proofHot: "maximum hot processes",
    proofRenderer: "active renderer",
    proofKeys: "model API keys held",
    installEyebrow: "VS CODE IS THE PC APP",
    installTitle: "Install once. Let runtrol find the CLIs.",
    installIntro: "There is no separate desktop window to manage. Runtrol Studio lives where the work already happens and discovers supported installed CLIs at runtime.",
    stepOneTitle: "Install Runtrol Studio",
    stepOneBody: "Install the signed extension from the Visual Studio Marketplace.",
    stepTwoTitle: "Open the Runtrol activity icon",
    stepTwoBody: "The session manager opens inside your current VS Code window.",
    stepThreeTitle: "Use the CLIs you already have",
    stepThreeBody: "Supported installations, versions, models, flags, and capabilities are discovered instead of hardcoded.",
    stepFourTitle: "Select a session and keep moving",
    stepFourBody: "The workspace follows your selection and cold sessions resume through their provider-native identity.",
    primaryInstall: "PRIMARY INSTALL",
    primaryInstallState: "Runtrol Studio is publicly available for supported native platforms.",
    openMarketplace: "Open Marketplace",
    manualInstall: "MANUAL FALLBACK",
    vsixUnavailable: "No public VSIX is available yet.",
    checkReleases: "Check Releases",
    downloadVsix: "Download VSIX",
    chooseVsix: "Choose platform",
    releaseChoose: "Choose the VSIX that matches your operating system and architecture.",
    phoneEyebrow: "THE SAME CORE, IN YOUR POCKET",
    phoneTitle: "Pair once. Keep the session owner on your PC.",
    phoneBody: "The phone PWA is live at this permanent HTTPS origin. It pairs as a scoped control surface and reconnects through an end-to-end encrypted relay without owning the session or receiving model API keys.",
    phoneProgress: "Secure phone app available",
    phoneHonesty: "Pair from the one-use QR in VS Code, then reopen it here.",
    phoneOpen: "Open phone app",
    principlesEyebrow: "THIN BY DESIGN",
    principlesTitle: "The supervisor stays out of the conversation.",
    principleOneTitle: "Provider-owned history",
    principleOneBody: "runtrol transports process events. It never keeps a second transcript or rewrites meaning.",
    principleTwoTitle: "Runtime discovery",
    principleTwoBody: "Providers, versions, models, flags, and session paths come from the installed CLI at runtime.",
    principleThreeTitle: "Remote default deny",
    principleThreeBody: "Phone control is scoped, paired, and encrypted. Model credentials never cross runtrol.",
    footerText: "Open source. Built around the sessions and tools you already own.",
  },
  ko: {
    navProduct: "제품",
    navInstall: "설치",
    navPhone: "휴대폰",
    heroEyebrow: "오픈소스 컨트롤 플레인",
    heroTitle: "하나의 VS Code 창.<br>모든 세션을 바로 곁에.",
    heroLede: "runtrol은 컴퓨터에 이미 설치된 코딩 에이전트 CLI를 감독합니다. VS Code를 느린 대화 보관소로 만들지 않고 저장소를 바꾸고, 공급자가 소유한 세션을 재개하며, 30개 대화를 정리합니다.",
    installMarketplace: "Marketplace에서 설치",
    viewSource: "소스 보기",
    releaseChecking: "GitHub Releases에서 VSIX를 확인하고 있습니다.",
    releaseMissing: "아직 공개 VSIX가 없습니다.",
    releaseFound: "GitHub Releases에서 수동 설치용 VSIX를 받을 수 있습니다.",
    sessions: "세션",
    selectedSession: "선택한 세션",
    workspaceFollows: "선택에 따라 워크스페이스 전환",
    proofSessions: "세션 출시 게이트",
    proofHot: "최대 활성 프로세스",
    proofRenderer: "활성 렌더러",
    proofKeys: "보유하는 모델 API 키",
    installEyebrow: "VS CODE가 PC 앱입니다",
    installTitle: "한 번 설치하면 CLI는 runtrol이 찾습니다.",
    installIntro: "관리할 별도 데스크톱 창은 없습니다. Runtrol Studio는 작업이 일어나는 VS Code 안에 있고 지원되는 설치 CLI를 런타임에 찾습니다.",
    stepOneTitle: "Runtrol Studio 설치",
    stepOneBody: "Visual Studio Marketplace에서 서명된 확장을 설치합니다.",
    stepTwoTitle: "Runtrol 활동 아이콘 열기",
    stepTwoBody: "현재 VS Code 창 안에서 세션 관리자가 열립니다.",
    stepThreeTitle: "이미 설치된 CLI 사용",
    stepThreeBody: "지원 설치, 버전, 모델, 플래그, 기능을 하드코딩하지 않고 찾아냅니다.",
    stepFourTitle: "세션을 선택하고 계속 작업",
    stepFourBody: "워크스페이스가 선택을 따라가고 콜드 세션은 공급자 고유 식별자로 재개됩니다.",
    primaryInstall: "기본 설치",
    primaryInstallState: "지원되는 네이티브 플랫폼용 Runtrol Studio가 공개되어 있습니다.",
    openMarketplace: "Marketplace 열기",
    manualInstall: "수동 설치 대안",
    vsixUnavailable: "아직 공개 VSIX가 없습니다.",
    checkReleases: "Releases 확인",
    downloadVsix: "VSIX 다운로드",
    chooseVsix: "플랫폼 선택",
    releaseChoose: "운영체제와 아키텍처에 맞는 VSIX를 선택하세요.",
    phoneEyebrow: "주머니 속에서도 같은 CORE",
    phoneTitle: "한 번 페어링하고 세션 소유자는 PC에 둡니다.",
    phoneBody: "휴대폰 PWA가 이 영구 HTTPS 주소에서 제공됩니다. 범위가 제한된 제어 화면으로 페어링하고, 세션을 소유하거나 모델 API 키를 받지 않은 채 종단간 암호화 릴레이로 같은 Core에 다시 연결합니다.",
    phoneProgress: "안전한 휴대폰 앱 사용 가능",
    phoneHonesty: "VS Code의 일회용 QR로 페어링한 뒤 여기서 다시 여세요.",
    phoneOpen: "휴대폰 앱 열기",
    principlesEyebrow: "의도적으로 얇게",
    principlesTitle: "감독자는 대화에 끼어들지 않습니다.",
    principleOneTitle: "공급자가 소유하는 기록",
    principleOneBody: "runtrol은 프로세스 이벤트만 운반합니다. 두 번째 대화 사본을 보관하거나 의미를 바꾸지 않습니다.",
    principleTwoTitle: "런타임 탐색",
    principleTwoBody: "공급자, 버전, 모델, 플래그, 세션 경로는 설치된 CLI에서 런타임에 얻습니다.",
    principleThreeTitle: "원격 기본 거부",
    principleThreeBody: "휴대폰 제어는 범위가 제한되고 페어링되며 암호화됩니다. 모델 자격증명은 runtrol을 통과하지 않습니다.",
    footerText: "오픈소스. 이미 소유한 세션과 도구를 중심으로 만듭니다.",
  },
  zh: {
    navProduct: "产品",
    navInstall: "安装",
    navPhone: "手机",
    heroEyebrow: "开源控制平面",
    heroTitle: "一个 VS Code 窗口。<br>所有会话触手可及。",
    heroLede: "runtrol 管理电脑上已经安装的编码代理 CLI。切换仓库，恢复由供应方保存的会话，并流畅整理 30 个对话，而不会把 VS Code 变成迟缓的聊天档案库。",
    installMarketplace: "从 Marketplace 安装",
    viewSource: "查看源码",
    releaseChecking: "正在 GitHub Releases 中检查 VSIX。",
    releaseMissing: "目前还没有公开 VSIX。",
    releaseFound: "GitHub Releases 已提供手动安装用 VSIX。",
    sessions: "会话",
    selectedSession: "已选会话",
    workspaceFollows: "工作区跟随选择",
    proofSessions: "会话发布门槛",
    proofHot: "最大活跃进程数",
    proofRenderer: "活跃渲染器",
    proofKeys: "持有的模型 API 密钥",
    installEyebrow: "VS CODE 就是桌面应用",
    installTitle: "安装一次，让 runtrol 自动寻找 CLI。",
    installIntro: "无需管理单独的桌面窗口。Runtrol Studio 就在工作的 VS Code 中，并在运行时发现受支持的已安装 CLI。",
    stepOneTitle: "安装 Runtrol Studio",
    stepOneBody: "从 Visual Studio Marketplace 安装已签名扩展。",
    stepTwoTitle: "打开 Runtrol 活动图标",
    stepTwoBody: "会话管理器会在当前 VS Code 窗口内打开。",
    stepThreeTitle: "使用现有 CLI",
    stepThreeBody: "自动发现受支持的安装、版本、模型、参数和能力，而不是硬编码。",
    stepFourTitle: "选择会话并继续工作",
    stepFourBody: "工作区会跟随选择，冷会话通过供应方原生身份恢复。",
    primaryInstall: "首选安装方式",
    primaryInstallState: "Runtrol Studio 已面向支持的原生平台公开发布。",
    openMarketplace: "打开 Marketplace",
    manualInstall: "手动安装备用方式",
    vsixUnavailable: "目前还没有公开 VSIX。",
    checkReleases: "查看 Releases",
    downloadVsix: "下载 VSIX",
    chooseVsix: "选择平台",
    releaseChoose: "请选择与操作系统和架构匹配的 VSIX。",
    phoneEyebrow: "口袋里的同一个 CORE",
    phoneTitle: "配对一次，让电脑继续拥有会话。",
    phoneBody: "手机 PWA 已在这个永久 HTTPS 地址提供。它作为权限受限的控制界面完成配对，并通过端到端加密中继重新连接同一个 Core，不拥有会话，也不接收模型 API 密钥。",
    phoneProgress: "安全手机应用现已可用",
    phoneHonesty: "先使用 VS Code 中的一次性二维码配对，然后从这里重新打开。",
    phoneOpen: "打开手机应用",
    principlesEyebrow: "刻意保持轻薄",
    principlesTitle: "管理器不会介入对话。",
    principleOneTitle: "供应方拥有历史记录",
    principleOneBody: "runtrol 只传输进程事件，不保存第二份对话，也不改写含义。",
    principleTwoTitle: "运行时发现",
    principleTwoBody: "供应方、版本、模型、参数和会话路径都在运行时从已安装的 CLI 获取。",
    principleThreeTitle: "远程默认拒绝",
    principleThreeBody: "手机控制经过限制、配对和加密。模型凭据永远不会经过 runtrol。",
    footerText: "开源。围绕你已经拥有的会话和工具构建。",
  },
  ja: {
    navProduct: "製品",
    navInstall: "インストール",
    navPhone: "スマートフォン",
    heroEyebrow: "オープンソースのコントロールプレーン",
    heroTitle: "ひとつの VS Code ウィンドウ。<br>すべてのセッションを手元に。",
    heroLede: "runtrol はマシンにすでにインストールされているコーディングエージェント CLI を監督します。VS Code を重いチャット保管庫にせず、リポジトリを切り替え、プロバイダー所有のセッションを再開し、30件の会話を整理できます。",
    installMarketplace: "Marketplace からインストール",
    viewSource: "ソースを見る",
    releaseChecking: "GitHub Releases で VSIX を確認しています。",
    releaseMissing: "公開 VSIX はまだありません。",
    releaseFound: "手動インストール用 VSIX を GitHub Releases から取得できます。",
    sessions: "セッション",
    selectedSession: "選択中のセッション",
    workspaceFollows: "選択に合わせてワークスペースを切り替え",
    proofSessions: "セッションのリリースゲート",
    proofHot: "最大ホットプロセス数",
    proofRenderer: "アクティブレンダラー",
    proofKeys: "保持するモデル API キー",
    installEyebrow: "VS CODE が PC アプリです",
    installTitle: "一度インストールすれば、CLI は runtrol が見つけます。",
    installIntro: "管理する別のデスクトップウィンドウはありません。Runtrol Studio は作業中の VS Code 内にあり、対応するインストール済み CLI を実行時に検出します。",
    stepOneTitle: "Runtrol Studio をインストール",
    stepOneBody: "Visual Studio Marketplace から署名済み拡張機能をインストールします。",
    stepTwoTitle: "Runtrol のアクティビティアイコンを開く",
    stepTwoBody: "現在の VS Code ウィンドウ内にセッションマネージャーが開きます。",
    stepThreeTitle: "すでにある CLI を使う",
    stepThreeBody: "対応するインストール、バージョン、モデル、フラグ、機能をハードコードせず検出します。",
    stepFourTitle: "セッションを選んで作業を続ける",
    stepFourBody: "ワークスペースが選択に追従し、コールドセッションはプロバイダー固有の識別子で再開します。",
    primaryInstall: "基本インストール",
    primaryInstallState: "対応するネイティブプラットフォーム向け Runtrol Studio を公開しています。",
    openMarketplace: "Marketplace を開く",
    manualInstall: "手動インストールの代替",
    vsixUnavailable: "公開 VSIX はまだありません。",
    checkReleases: "Releases を確認",
    downloadVsix: "VSIX をダウンロード",
    chooseVsix: "プラットフォームを選択",
    releaseChoose: "OS とアーキテクチャに合う VSIX を選択してください。",
    phoneEyebrow: "ポケットの中でも同じ CORE",
    phoneTitle: "一度ペアリングし、セッション所有者は PC に残します。",
    phoneBody: "スマートフォン PWA はこの恒久的な HTTPS オリジンで利用できます。範囲を限定した操作画面としてペアリングし、セッションを所有せずモデル API キーも受け取らずに、エンドツーエンド暗号化リレーで同じ Core へ再接続します。",
    phoneProgress: "安全なスマートフォンアプリを利用可能",
    phoneHonesty: "VS Code の一回限りの QR でペアリングしてから、ここで再度開いてください。",
    phoneOpen: "スマートフォンアプリを開く",
    principlesEyebrow: "意図的に薄く",
    principlesTitle: "監督役は会話に介入しません。",
    principleOneTitle: "プロバイダー所有の履歴",
    principleOneBody: "runtrol はプロセスイベントだけを運びます。会話の複製を保存せず、意味を書き換えません。",
    principleTwoTitle: "実行時の検出",
    principleTwoBody: "プロバイダー、バージョン、モデル、フラグ、セッションパスはインストール済み CLI から実行時に取得します。",
    principleThreeTitle: "リモートは既定で拒否",
    principleThreeBody: "スマートフォン操作は範囲が限定され、ペアリングされ、暗号化されます。モデル認証情報が runtrol を通ることはありません。",
    footerText: "オープンソース。すでに所有しているセッションとツールを中心に構築します。",
  },
};

const state = {
  locale: "en",
  releaseChecked: false,
  vsixUrl: null,
  releaseUrl: null,
};

async function inferBrowserTarget() {
  let architecture = "";
  let bitness = "";
  try {
    if (navigator.userAgentData?.getHighEntropyValues) {
      const values = await navigator.userAgentData.getHighEntropyValues(["architecture", "bitness"]);
      architecture = values.architecture ?? "";
      bitness = values.bitness ?? "";
    }
  } catch {
    // A matching Marketplace install remains the primary path when hints are unavailable.
  }
  return inferNativeTarget({
    userAgentDataPlatform: navigator.userAgentData?.platform,
    platform: navigator.platform,
    userAgent: navigator.userAgent,
    architecture,
    bitness,
  });
}

function readPreference(key) {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}

function writePreference(key, value) {
  try {
    localStorage.setItem(key, value);
  } catch {
    // Preferences remain optional when browser storage is unavailable.
  }
}

function copyFor(key) {
  return COPY[state.locale][key] ?? COPY.en[key] ?? "";
}

function applyLocale(locale) {
  state.locale = Object.hasOwn(COPY, locale) ? locale : "en";
  document.documentElement.lang = state.locale;
  document.querySelectorAll("[data-i18n]").forEach((element) => {
    const value = copyFor(element.dataset.i18n);
    if (value) {
      element.innerHTML = value;
    }
  });
  document.querySelector("#language").value = state.locale;
  writePreference("runtrol-locale", state.locale);
  updateReleaseUi();
}

function applyTheme(theme) {
  const next = theme === "light" ? "light" : "dark";
  document.documentElement.dataset.theme = next;
  const toggle = document.querySelector("#theme-toggle");
  toggle.querySelector("span").textContent = next === "dark" ? "Light" : "Dark";
  toggle.setAttribute("aria-label", next === "dark" ? "Switch to light theme" : "Switch to dark theme");
  writePreference("runtrol-theme", next);
}

function updateReleaseUi() {
  const status = document.querySelector("#release-status");
  const description = document.querySelector("#vsix-description");
  const download = document.querySelector("#vsix-download");

  if (!state.releaseChecked) {
    status.textContent = copyFor("releaseChecking");
    return;
  }

  if (state.vsixUrl || state.releaseUrl) {
    status.textContent = copyFor("releaseFound");
    description.textContent = copyFor(state.vsixUrl ? "releaseFound" : "releaseChoose");
    download.textContent = copyFor(state.vsixUrl ? "downloadVsix" : "chooseVsix");
    download.href = state.vsixUrl ?? state.releaseUrl;
    download.classList.remove("is-disabled");
    download.removeAttribute("aria-disabled");
    return;
  }

  status.textContent = copyFor("releaseMissing");
  description.textContent = copyFor("vsixUnavailable");
  download.textContent = copyFor("checkReleases");
  download.href = `${PROJECT.repository}/releases`;
  download.classList.add("is-disabled");
  download.setAttribute("aria-disabled", "true");
}

async function discoverRelease() {
  try {
    const response = await fetch(PROJECT.releasesApi, {
      headers: { Accept: "application/vnd.github+json" },
    });
    if (!response.ok) {
      throw new Error(`release lookup returned ${response.status}`);
    }
    const release = await response.json();
    const assets = Array.isArray(release.assets) ? release.assets : [];
    const target = await inferBrowserTarget();
    const asset = selectTargetVsix(assets, target);
    const hasVsix = assets.some((candidate) => typeof candidate?.name === "string" && candidate.name.endsWith(".vsix"));
    state.vsixUrl = asset?.browser_download_url ?? null;
    state.releaseUrl = hasVsix && typeof release.html_url === "string" ? release.html_url : null;
  } catch {
    state.vsixUrl = null;
    state.releaseUrl = null;
  }
  state.releaseChecked = true;
  updateReleaseUi();
}

function initialLocale() {
  const stored = readPreference("runtrol-locale");
  if (stored && Object.hasOwn(COPY, stored)) {
    return stored;
  }
  return "en";
}

function initialTheme() {
  const stored = readPreference("runtrol-theme");
  if (stored === "light" || stored === "dark") {
    return stored;
  }
  return window.matchMedia("(prefers-color-scheme: light)").matches ? "light" : "dark";
}

document.querySelector("#language").addEventListener("change", (event) => applyLocale(event.target.value));
document.querySelector("#theme-toggle").addEventListener("click", () => {
  applyTheme(document.documentElement.dataset.theme === "dark" ? "light" : "dark");
});
document.querySelector("#vsix-download").addEventListener("click", (event) => {
  if (!state.vsixUrl && !state.releaseUrl) {
    event.preventDefault();
    window.location.href = `${PROJECT.repository}/releases`;
  }
});

applyTheme(initialTheme());
applyLocale(initialLocale());
discoverRelease();
