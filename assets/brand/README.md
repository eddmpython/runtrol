# 브랜드 자산

**정본은 기하와 `render.py` 다.** 이 폴더의 모든 SVG·PNG·ICO 는 `python -X utf8 assets/brand/render.py` 한 번으로 다시 나온다 (의존성 없음, 같은 입력이면 바이트까지 같다). 파일을 손으로 고치지 않는다. 기하나 색을 바꾸려면 `render.py` 의 표를 고치고 다시 돌린다. 예외는 `wordmark.svg` (윤곽선 원본, 트레이싱 산출물) 와 `tray-16-*.png` (단색 실루엣) 다.

## 색

| 이름 | 값 | 어디에 |
|---|---|---|
| Coral | `#F56565` | 마크의 액센트 팔 두 개 (좌상·우하), 버튼, 강조 |
| Graphite | `#0B0D0F` | 라이트 표면의 잉크 팔과 글자, 다크 타일의 바탕 |
| White | `#FFFFFF` | 다크 표면의 잉크 팔과 글자 |

**마크는 두 톤이다.** 네 팔 중 두 개가 코럴이고 두 개가 잉크다. 잉크는 표면을 따라간다: 라이트 배경에선 그래파이트, 다크 배경에선 화이트. 그래서 색이 있는 자리는 파일이 둘 (`-light` / `-dark`) 이거나, CSS `currentColor` 를 쓰는 인라인 하나다.

## 파일

### 벡터

| 파일 | 색 | 용도 |
|---|---|---|
| [symbol.svg](symbol.svg) | 전부 `currentColor` | 단색 마크. 에디터 활동 막대·아이콘 폰트처럼 한 색만 허용되는 자리 |
| [symbol-light.svg](symbol-light.svg) | 코럴 + 그래파이트 | `<img>` 로 넣는 라이트 배경 |
| [symbol-dark.svg](symbol-dark.svg) | 코럴 + 화이트 | `<img>` 로 넣는 다크 배경 |
| [favicon.svg](favicon.svg) | 코럴 + 잉크 (내장 media query 로 탭 테마를 따라감) | 브라우저 탭. **작은 크기용으로 따로 그린 기하** (아래) |
| [lockup.svg](lockup.svg) | 마크 코럴 + `currentColor`, 글자 `currentColor` | 인라인으로 넣을 수 있으면 이 하나로 테마 둘 다 된다 |
| [lockup-light.svg](lockup-light.svg) | 코럴 + 그래파이트 | 라이트 배경 |
| [lockup-dark.svg](lockup-dark.svg) | 코럴 + 화이트 | 다크 배경 |
| [wordmark.svg](wordmark.svg) | `currentColor` | 글자 단독. 윤곽선이지 폰트가 아니다 |

### 래스터

| 파일 | 크기 | 무엇 |
|---|---|---|
| [favicon.ico](favicon.ico) | 16 · 32 · 48 | 힌트 기하, 투명 바탕, 코럴 + 그래파이트 (탭 스트립은 대개 밝다) |
| [icon-16.png](icon-16.png) · [icon-32.png](icon-32.png) | 16 · 32 | 같은 것 |
| [icon-192.png](icon-192.png) · [icon-512.png](icon-512.png) | 192 · 512 | 그래파이트 정사각 타일 + 코럴·화이트 마크 64%. PWA manifest 와 Marketplace 아이콘. 어느 배경에서도 같은 얼굴이 필요해 불투명이다 |
| [apple-touch-icon.png](apple-touch-icon.png) | 180 | 같은 타일. iOS 는 투명을 검게 칠하므로 어차피 불투명이어야 한다 |
| [tray-16-light.png](tray-16-light.png) · [tray-16-dark.png](tray-16-dark.png) | 16 | Windows 트레이 단색 실루엣. `render.py` 가 만들지 않는다 |
| [social-card.png](social-card.png) · [social-card-dark.png](social-card-dark.png) | 1200x630 | 락업 가운데 정렬. Open Graph |

## 좌표계

`symbol.svg` 는 100x100 박스이고 마크가 박스에 꽉 찬다 (여백 0). 네 팔의 끝은 박스 경계에서 **평평하게** 잘린다 (butt cap). 여백은 쓰는 쪽이 준다.

| 값 | 단위 |
|---|---:|
| 획 두께 | 14 |
| 회전 반지름 (중심선 기준) | 20 |
| 바 중심선 | 39.5 · 60.5 (박스 중앙에서 10.5) |
| 중앙 틈 | 7 |

이 숫자는 로고 시트 원본에 최소자승 적합을 돌려 얻은 값 (획 13.94, 반지름 20.09, 중심선 39.55, 잔차 rms 0.16) 을 정수로 스냅한 것이다. `render.py` 의 `MARK` 가 이 표다.

락업은 같은 단위를 쓴다. 마크 100, 간격 33.14, 글자 345.09 x 70.35, 전체 479.07 x 100. 글자는 세로 가운데 (y = 14.825).

## favicon 이 symbol 과 다른 이유

16px 에서 진짜 기하는 획이 2.24px, 중앙 틈이 1.12px 다. 모든 경계가 픽셀 중간에 떨어져 마크가 회색으로 뭉갠다. 그래서 `favicon.svg` 와 16·32·48px 래스터는 획 12.5, 반지름 18.75, 중심선 37.5 (`render.py` 의 `HINTED`) 로 그린다. 이러면 16·32·48px 에서 획이 정확히 2·4·6px, 틈이 2·4·6px, 반지름이 3·6·9px 로 픽셀 격자에 앉는다.

**이 차이는 실수가 아니다.** 33px 이상은 진짜 기하가 알아서 또렷하다.

## 여백과 최소 크기

- **여백**: 마크 높이의 1/3 을 사방에. 그 안에 다른 요소를 넣지 않는다.
- **최소 크기**: 락업은 높이 16px, 마크 단독은 16x16px.

## 붙이는 법

```html
<link rel="icon" href="/assets/brand/favicon.svg" type="image/svg+xml">
<link rel="icon" href="/assets/brand/favicon.ico" sizes="48x48">
<link rel="apple-touch-icon" href="/assets/brand/apple-touch-icon.png">
<meta property="og:image" content="https://eddmpython.github.io/runtrol/assets/brand/social-card-dark.png">
<meta name="twitter:card" content="summary_large_image">
```

테마 따라 갈리는 마크는 파일 두 개로:

```html
<picture>
  <source srcset="/assets/brand/symbol-dark.svg" media="(prefers-color-scheme: dark)">
  <img src="/assets/brand/symbol-light.svg" alt="runtrol" height="32">
</picture>
```

SVG 를 인라인으로 넣을 수 있으면 `lockup.svg` 나 `symbol.svg` 하나면 된다. 잉크가 `currentColor` 라 `color` 만 상속시키면 테마가 따라온다. 인라인에서 두 톤을 내려면 `.accent` 팔에 코럴을, `.ink` 팔에 `currentColor` 를 준다 (`site/index.html` 헤더가 그 예다).

## 하지 말 것

- 비율·획 두께·중앙 틈을 바꾸지 않는다
- 마크를 회전하지 않는다 (4 회 회전 대칭이라 90 도는 무의미하고 45 도는 다른 로고가 된다)
- 코럴 팔의 위치 (좌상·우하) 를 바꾸지 않는다. 단색이 필요하면 `symbol.svg` 를 `currentColor` 로 쓴다
- 마크와 글자 사이 간격을 바꾸지 않는다
- 그림자·그라디언트·외곽선·글로우를 더하지 않는다
- 글자를 비슷한 폰트로 다시 짜지 않는다
- 래스터를 손으로 편집하지 않는다. `render.py` 를 돌린다

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
