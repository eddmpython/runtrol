# 브랜드 자산

**정본은 SVG 다.** PNG 와 ICO 는 SVG 를 못 받는 자리 (구형 브라우저 파비콘, iOS 홈화면, Windows 트레이, 소셜 카드) 를 위한 파생물이고, 전부 아래 벡터에서 나온다.

## 색

| 이름 | 값 | 어디에 |
|---|---|---|
| Orange | `#FF5A2F` | 마크. 라이트·다크 양쪽에서 그대로 |
| Graphite | `#0B0D0F` | 라이트 배경 위의 글자 |
| White | `#FFFFFF` | 다크 배경 위의 글자 |

**마크에는 테마 변형이 없다.** 오렌지가 흰 배경과 검은 배경 양쪽에서 읽히기 때문이고, 그래서 파비콘도 파일이 하나다. 테마에 따라 갈리는 것은 **글자색뿐**이다.

## 파일

### 벡터 (정본)

| 파일 | 색 | 용도 |
|---|---|---|
| [symbol.svg](symbol.svg) | `currentColor` | 마크 단독. CSS `color` 가 색을 정한다. 기본으로 이것을 쓴다 |
| [symbol-orange.svg](symbol-orange.svg) | 오렌지 고정 | 색을 상속할 수 없는 자리 (`<img>`, README 임베드) |
| [wordmark.svg](wordmark.svg) | `currentColor` | 글자 단독 |
| [lockup.svg](lockup.svg) | 마크 오렌지 + 글자 `currentColor` | 인라인으로 넣을 수 있으면 이 하나로 테마 둘 다 된다 |
| [lockup-light.svg](lockup-light.svg) | 글자 그래파이트 | 라이트 배경 |
| [lockup-dark.svg](lockup-dark.svg) | 글자 화이트 | 다크 배경 |
| [favicon.svg](favicon.svg) | 오렌지 고정 | 브라우저 탭. **작은 크기용으로 따로 그린 것이다** (아래 참고) |

### 래스터 (파생)

| 파일 | 크기 | 무엇에서 나왔나 |
|---|---|---|
| [favicon.ico](favicon.ico) | 16 · 32 · 48 | `favicon.svg`. 세 크기를 각자 렌더한 것이지 큰 것을 줄인 게 아니다 |
| [icon-16.png](icon-16.png) · [icon-32.png](icon-32.png) | 16 · 32 | `favicon.svg` |
| [icon-192.png](icon-192.png) · [icon-512.png](icon-512.png) | 192 · 512 | `symbol-orange.svg`. PWA manifest |
| [apple-touch-icon.png](apple-touch-icon.png) | 180 | 그래파이트 정사각 + 오렌지 마크 64%. iOS 는 투명을 검게 칠하므로 불투명이어야 한다 |
| [tray-16-light.png](tray-16-light.png) · [tray-16-dark.png](tray-16-dark.png) | 16 | Windows 트레이 단색 실루엣. 밝은 작업표시줄엔 light, 어두운 쪽엔 dark |
| [social-card.png](social-card.png) · [social-card-dark.png](social-card-dark.png) | 1200x630 | Open Graph |

## 좌표계

`symbol.svg` 는 100x100 박스이고 마크가 박스에 꽉 찬다 (여백 0). 네 팔의 끝은 박스 경계에서 **평평하게** 잘린다 (butt cap). 여백은 쓰는 쪽이 준다.

| 값 | 단위 |
|---|---:|
| 획 두께 | 14 |
| 회전 반지름 (중심선 기준) | 20 |
| 바 중심선 | 39.5 · 60.5 (박스 중앙에서 10.5) |
| 중앙 틈 | 7 |

이 숫자는 로고 시트 원본에 최소자승 적합을 돌려 얻은 값 (획 13.94, 반지름 20.09, 중심선 39.55, 잔차 rms 0.16) 을 정수로 스냅한 것이다. **마크를 다시 그려야 하면 원본 이미지가 아니라 이 표가 정본이다.**

락업은 같은 단위를 쓴다. 마크 100, 간격 33.14, 글자 345.09 x 70.35, 전체 479.07 x 100. 글자는 세로 가운데 (y = 14.825).

글자는 **윤곽선이지 폰트가 아니다.** 폰트 파일 의존이 없고, 대신 다시 조판할 수도 없다.

## favicon.svg 가 symbol.svg 와 다른 이유

16px 에서 진짜 기하는 획이 2.24px, 중앙 틈이 1.12px 다. 모든 경계가 픽셀 중간에 떨어져 마크가 회색으로 뭉갠다. 그래서 `favicon.svg` 와 16·32px PNG 는 획 12.5, 반지름 18.75, 중심선 37.5 로 다시 그렸다. 이러면 16·32·48px 에서 획이 정확히 2·4·6px, 틈이 2·4·6px, 반지름이 3·6·9px 로 픽셀 격자에 앉는다.

**이 차이는 실수가 아니다.** 두 파일을 같게 만들지 않는다. 33px 이상은 진짜 기하가 알아서 또렷하다.

## 여백과 최소 크기

- **여백**: 마크 높이의 1/3 을 사방에. 그 안에 다른 요소를 넣지 않는다.
- **최소 크기**: 락업은 높이 16px, 마크 단독은 16x16px. 그 아래로는 쓰지 않는다.

## 붙이는 법

```html
<link rel="icon" href="/assets/brand/favicon.svg" type="image/svg+xml">
<link rel="icon" href="/assets/brand/favicon.ico" sizes="48x48">
<link rel="apple-touch-icon" href="/assets/brand/apple-touch-icon.png">
<meta property="og:image" content="https://runtrol.dev/assets/brand/social-card.png">
<meta name="twitter:card" content="summary_large_image">
```

테마 따라 갈리는 락업은 파일 두 개로:

```html
<picture>
  <source srcset="/assets/brand/lockup-dark.svg" media="(prefers-color-scheme: dark)">
  <img src="/assets/brand/lockup-light.svg" alt="runtrol" height="32">
</picture>
```

SVG 를 인라인으로 넣을 수 있으면 `lockup.svg` 하나면 된다. 글자가 `currentColor` 라 `color` 만 상속시키면 테마가 따라온다.

## 하지 말 것

- 비율·획 두께·중앙 틈을 바꾸지 않는다
- 마크를 회전하지 않는다 (4 회 회전 대칭이라 90 도는 무의미하고 45 도는 다른 로고가 된다)
- 오렌지를 다른 색으로 갈지 않는다. 단색이 필요하면 `symbol.svg` 를 `currentColor` 로 쓴다
- 마크와 글자 사이 간격을 바꾸지 않는다
- 그림자·그라디언트·외곽선·글로우를 더하지 않는다
- 글자를 비슷한 폰트로 다시 짜지 않는다

## Provider marks (`provider-icons/`)

Each coding service's own mark, as a vector, monochrome, following the editor theme. A raster wrapped in
an SVG is refused by `tests/audit/vscodeExtension.py` (it blurs at sidebar sizes and cannot follow the theme).

| File | What | Source | Licence |
|---|---|---|---|
| `claude.svg` | Anthropic's Claude mark | vendor site | trademark of Anthropic |
| `openai.svg` | OpenAI's 2025 symbol | Wikimedia Commons `OpenAI logo 2025 (symbol).svg`, derived from OpenAI's own logo file | public domain as simple geometry; trademark of OpenAI |
| `cline.svg` | Cline's mark | `cline/cline` repository, `apps/vscode/assets/icons/icon.svg` | Apache-2.0; trademark of Cline |
| `opencode.svg` | OpenCode's mark | vendor site | trademark of its owner |
| `grok.svg` | Grok's mark since February 2025 (the xAI strokes) | https://x.ai/, geometry as Wikimedia Commons `XAI-Logo.svg` | public domain as simple geometry; trademark of xAI |

The 2023 Grok mark (a slashed circle) was retired by the vendor in February 2025; the sidebar carried it
as a black app tile until 2026-08-25.
