# 프론트 스택 . Astryx + StyleX

운영자 지시 (2026-07-30): **"프론트는 astryx 방식. xlpod 을 참고하라."**

세 표면 (랜딩 · PWA · 데스크톱 GUI) 이 공유하는 결정이므로 여기가 정본이다. 각 이니셔티브는 이 문서를 가리킨다.

## 무엇인가

| 패키지 | 버전 | 라이선스 | 무엇 |
|---|---|---|---|
| `@astryxdesign/core` | 0.1.4 | MIT | React 컴포넌트 라이브러리 **103 개**. 접근성·간격·다크모드가 내장돼 있다 |
| `@astryxdesign/theme-neutral` | 0.1.4 | MIT | 테마. *"Restrained warm grays. Minimal and quiet, so the content stays the focus."* |
| `@stylexjs/stylex` | (Meta) | MIT | Astryx 의 스타일 엔진. [facebook/stylex](https://github.com/facebook/stylex) |

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

**번들에 벤더링한다. CDN 을 부르지 않는다.** [pwaConnection](../mainPlan/pwaConnection/README.md) 의 CSP 원칙, GitHub Pages 정적 호스팅, 오프라인 동작이 전부 같은 방향을 가리킨다.

## 테마

`<html>` 의 `data-astryx-media` 속성으로 전환한다.

```js
document.documentElement.dataset.astryxMedia = loadTheme();  // "light" | "dark"
```

- localStorage 에 보존하고, 저장소가 막혀 있으면 (시크릿 모드·임베드) **light 로 안정적으로 떨어진다**
- 정본 이름은 `"light"` 와 `"dark"` 둘뿐. 그 밖의 값은 받지 않는다
- 테마 토글은 저장소가 없어도 **동작은 계속해야 한다**

## runtrol 이 쓸 컴포넌트

103 개 중 우리 화면에 바로 붙는 것들이다. **이 매핑이 중요한 이유: 우리가 새로 디자인해야 할 것이 거의 없다.**

| runtrol 화면 | Astryx 컴포넌트 |
|---|---|
| 앱 껍데기 (좌측 세션 목록 + 본문) | `AppShell` · `SideNav` · `TopNav` |
| 세션 목록 | `List` · `Card` · `Badge` (provider 뱃지) · `StatusDot` (hot/warm/cold, 실행중/대기) |
| 대화 표면 | `Chat` · `Markdown` · `CodeBlock` |
| 빠른 세션 전환 | `CommandPalette` |
| 승인 카드 | `AlertDialog` · `Dialog` · `Toast` |
| 입력 | `TextArea` · `TextInput` · `Button` · `ButtonGroup` |
| 설정 | `Switch` · `ToggleButton` |
| 레이아웃·타이포 | `Stack` · `VStack` · `Text` · `Tooltip` |

**주의 하나: `Skeleton` 을 남용하지 않는다.** [desktopGui](../mainPlan/desktopGui/README.md) 의 성능 계약은 "스켈레톤이 아니라 이미 가진 꼬리를 먼저 그린다" 이다. 스켈레톤은 정말 아무것도 없을 때만 쓴다. **로딩을 예쁘게 보여주는 것은 편의가 아니다. 로딩이 없는 것이 편의다.**

## 이 결정의 구조적 파급

Astryx 가 **React** 컴포넌트라는 사실이 스택 선택에 직접 영향을 준다.

**랜딩 · PWA · 데스크톱 GUI 셋이 같은 컴포넌트 층을 공유할 수 있다.** 그러면 북극성 `공급자 확장성` 축의 *"새 CLI 가 나와도 PC 화면과 폰 화면과 조작 방법은 그대로다"* 가 규율이 아니라 **구조로** 보장된다. 한 곳을 고치면 세 곳이 같이 바뀐다.

이것은 [desktopGui](../mainPlan/desktopGui/README.md) 의 GUI 스택 선택에서 **Tauri v2 쪽으로 무게를 크게 옮긴다** (egui·iced 를 고르면 데스크톱만 다른 컴포넌트 세계가 되고, 그 순간 위 보장이 사라진다).

**다만 아직 확정하지 않는다.** 판정 1 순위는 여전히 **한글 IME 와 텍스트 선택**이고, 그 다음이 프레임 예산과 유휴 RSS 다. 컴포넌트 공유는 강한 이점이지만 편의의 바닥 조건을 이기지는 못한다. 프로토타입 실측 (`tests/_attempts/desktopShell/`) 후에 정한다.

## 게이트

- `frontendVendored`: 런타임에 CDN 을 부르지 않는다 (CSP 와 오프라인 동작의 전제)
- `themeContract`: `data-astryx-media` 가 `light`/`dark` 둘만 받고, 저장소가 막혀도 토글이 동작한다
- `landingBudget`: 랜딩 번들 예산. 랜딩이 무거우면 "상주 비용" 을 말할 자격이 없다
