# 프론트 스택 . Astryx + StyleX

운영자 지시 (2026-07-30): **"프론트는 astryx 방식. xlpod 을 참고하라."**

세 표면 (랜딩 · PWA · 데스크톱 GUI) 이 공유하는 결정이므로 여기가 정본이다. 각 이니셔티브는 이 문서를 가리킨다.

## 무엇인가

| 패키지 | 버전 | 라이선스 | 무엇 |
|---|---|---|---|
| `@astryxdesign/core` | 0.1.4 | MIT | React 컴포넌트 라이브러리 **103 개**. 접근성·간격·다크모드가 내장돼 있다 |
| `@astryxdesign/theme-neutral` | 0.1.4 | MIT | 테마. *"Restrained warm grays. Minimal and quiet, so the content stays the focus."* |
| `@stylexjs/stylex` | 0.18.3 | MIT | Astryx 의 스타일 엔진. [facebook/stylex](https://github.com/facebook/stylex) |

**MIT 라 runtrol (MIT) 과 호환된다.** `NOTICE` 에 고지한다.

theme-neutral 의 한 줄 설명이 runtrol 브랜드 방향과 정확히 같은 방향이다: 조용하고, 내용이 주인공이고, 도구가 안 보인다.

## 소비 방식 (xlpod 실물 승계)

진입점에서 CSS 세 개를 이 **순서대로** 싣는다.

```js
import "@astryxdesign/core/reset.css";
import "@astryxdesign/core/astryx.css";
import "@astryxdesign/theme-neutral/theme.css";
// 그 다음 폰트, 그 다음 우리 스타일
```

**StyleX 컴파일러 플러그인은 붙이지 않는다.** Astryx 컴포넌트는 이미 컴파일돼 `astryx.css` 안에 스타일이 있고, `@stylexjs/stylex` 는 런타임 className 병합만 한다. **우리가 StyleX 로 직접 스타일을 저작할 때 그때 컴파일러를 붙인다.** (xlpod 이 소스 주석으로 남긴 판단이고 그대로 따른다.)

**번들에 벤더링한다. CDN 을 부르지 않는다.** production desktop bundle 은 네트워크 없이 같은 CSS 와 컴포넌트를 불러오며, 이 경계는 정적 호스팅과 오프라인 표면에도 그대로 적용한다.

## 테마

Astryx `Theme` 에 `neutralTheme` 과 mode 를 주고, `<html>` 의 `data-theme` 과 `data-astryx-media` 를 같은 값으로 전환한다.

```js
const mode = initialTheme();  // "light" | "dark"
document.documentElement.dataset.theme = mode;
document.documentElement.dataset.astryxMedia = mode;
```

- localStorage 에 보존하고, 값이 없거나 저장소가 막혀 있으면 운영체제의 `prefers-color-scheme` 을 따른다
- 정본 이름은 `"light"` 와 `"dark"` 둘뿐. 그 밖의 값은 받지 않는다
- 테마 토글은 저장소가 없어도 **동작은 계속해야 한다**

## runtrol 이 쓸 컴포넌트

103 개 중 우리 화면에 바로 붙는 것들이다. **이 매핑이 중요한 이유: 우리가 새로 디자인해야 할 것이 거의 없다.**

| 현재 desktop 화면 | Astryx 컴포넌트 |
|---|---|
| 앱 껍데기 | `Theme` · `AppShell` |
| 세션 목록과 검색 | `SideNav` · `SideNavSection` · `SideNavItem` · `SideNavHeading` · `TextInput` · `Badge` · `StatusDot` |
| 대화와 입력 | `ChatLayout` · `ChatMessageList` · `ChatMessage` · `ChatMessageBubble` · `ChatSystemMessage` · `ChatComposer` · `ChatSendButton` |
| 세션 시작 | `Dialog` · `DialogHeader` · `Layout` · `LayoutContent` · `LayoutFooter` · `Selector` · `TextInput` |
| 확인과 상태 | `AlertDialog` · `EmptyState` · `Text` · `Button` |

**주의 하나: `Skeleton` 을 남용하지 않는다.** [desktop GUI](desktopGui.md) 의 성능 계약은 "스켈레톤이 아니라 이미 가진 꼬리를 먼저 그린다" 이다. 스켈레톤은 정말 아무것도 없을 때만 쓴다. **로딩을 예쁘게 보여주는 것은 편의가 아니다. 로딩이 없는 것이 편의다.**

## 이 결정의 구조적 파급

Astryx 가 **React** 컴포넌트라는 사실이 스택 선택에 직접 영향을 준다.

**랜딩 · PWA · 데스크톱 GUI 셋이 같은 컴포넌트 층을 공유할 수 있다.** 데스크톱은 이 계층을 실제로 소비한다. 랜딩과 PWA 는 구현될 때 같은 패키지와 theme contract 를 재사용하되, 아직 존재하지 않는 공유 화면을 구현된 것으로 주장하지 않는다.

데스크톱 셸은 Tauri v2 웹뷰로 확정됐으며 그 운영 계약은 [desktop GUI](desktopGui.md) 가 정본이다. PWA 화면 공유는 그 이니셔티브가 구현될 때 production component boundaries 를 기준으로 결정한다.

이 공유 계층은 Tauri v2 안에서 production bundle 로 동작하며 랜딩과 PWA 가 같은 컴포넌트 계약을 소비할 수 있게 한다.

선택은 실제 Windows 한글 IME와 텍스트 선택, 프레임 예산, GUI 및 WebView2 메모리 계약을 통과한 뒤 확정됐다. Linux와 macOS의 실제 창, OS IME, GUI process-tree 증거는 아직 주장하지 않는다.

## 게이트

- `frontendBuild`: TypeScript checking and the production Vite build succeed with vendored dependencies.
- `desktopThinBoundary`: only theme and last-provider scalar preferences may use browser storage, and the page has no filesystem capability.
- `desktopTextInputSmoke`: the production bundle preserves composition, selection, copy, token nodes, and lifecycle resets in a real browser.
- `desktopLifecycleSmoke`: the production bundle keeps the provider-neutral session surface responsive across start, preparation, send, and confirmed removal.
