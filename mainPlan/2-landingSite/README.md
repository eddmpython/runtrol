# landingSite

상태: 설계 완료, 착수 순서 2 번. 로고 도착 (2026-07-30). 자산은 [`assets/brand/`](../../assets/brand/) 에 있고 그 폴더의 README 가 사용 규칙의 정본이다.

**왜 3 번 앞인가.** 다운로드 링크를 릴리즈에서 파생하므로 [1-autoUpdate](../1-autoUpdate/README.md) 가 선행이고,
더 중요하게는 **이것이 도메인을 확정한다.** [3-pwaConnection](../3-pwaConnection/README.md) 의 첫 번째 결정이
"origin 은 영원히 안 바뀐다" 이고 그 문서의 운영자 대기 1 번이 "도메인 하나를 영원히 소유할 것인가" 다.
주소를 소유하기 전에 전송 계층을 지으면 그 위에 세운 기기 신원과 push 구독과 설치가 주소와 함께 날아간다.

## 한 문장 정의

**GitHub Pages 에 뜨는 간단한 렌더링 한 장.** 로고와 설명, 다운로드 둘 (PC exe 런처 · 모바일 PWA), 우측 상단 SNS. 그 이상 안 만든다.

## 화면 구성

```
+--------------------------------------------------------------+
|  [로고] runtrol                        [테마] [SNS 아이콘들]  |  <- 우측 상단 SNS
|                                                              |
|                여러 AI를 한 곳에서 관리한다                    |  <- 한 줄 북극성
|              (한 문단 설명. 무엇이고 왜 필요한가)               |
|                                                              |
|      [ PC 다운로드 (Windows) ]    [ 모바일 PWA 열기 ]          |  <- 다운로드 둘
|                                                              |
|              (되는 것 몇 줄 · 안 하는 것 몇 줄)                 |
+--------------------------------------------------------------+
```

**언어는 README 와 같은 넷** (한국어·English·中文·日本語). 세계화가 목표다.

## 우측 상단 SNS . xlpod 방식 승계

xlpod `ui/chrome/bars/brandSocial.jsx` 의 구조를 그대로 가져온다.

- `nav.sns` 안에 작은 버튼들 (15x15 인라인 SVG, 외부 아이콘 폰트·CDN 없음)
- **링크는 한 곳에서만 정의한다** (xlpod 의 `brand.js` `LINKS` 처럼). 화면 여러 곳에 URL 을 흩지 않는다
- 순서: 테마 토글 -> Threads -> YouTube -> Gmail -> GitHub
- 각 항목에 `title` 과 `aria-label` 을 붙인다 (접근성)
- 외부 링크는 `target="_blank" rel="noopener"`

**아이콘은 인라인 SVG 다.** CDN 을 부르지 않는 것이 GitHub Pages 정적 호스팅과도 맞고, [pwaConnection](../3-pwaConnection/README.md) 의 CSP 원칙과도 같은 방향이다.

## 다운로드 . 릴리즈에서 파생한다

**버전 번호를 손으로 적지 않는다.** 손으로 적은 열거는 반드시 썩는다.

- PC 버튼은 **GitHub Releases API** 로 최신 릴리즈의 Windows 아티팩트를 찾아 링크한다
- 릴리즈가 없으면 버튼이 "준비 중" 으로 정직하게 뜬다 (죽은 링크를 두지 않는다)
- 모바일 버튼은 PWA origin 으로 간다. **origin 은 영원히 안 바뀐다** ([pwaConnection](../3-pwaConnection/README.md) 의 첫 번째 결정)

게이트: `landingLinksAlive` (다운로드 링크가 실제로 200 이거나 정직하게 "준비 중")

## 원칙

- **한 장이다.** 라우팅·블로그·문서 사이트를 여기 만들지 않는다. 문서는 저장소의 `docs/` 가 정본이다
- **빌드 산출물이 가볍다.** 랜딩이 무거우면 "상주 비용" 을 말할 자격이 없다. 예산을 게이트로 잠근다 (`landingBudget`)
- **JS 없이도 읽힌다.** 로고·설명·링크는 정적 HTML 이고, 릴리즈 조회만 JS 다
- **다크·라이트 둘 다.** 시스템 설정을 따르고 토글을 준다

## 프론트 . Astryx + StyleX

**정본은 [docs/frontendStack.md](../../docs/frontendStack.md) 다.** 여기 되풀이하지 않는다.

랜딩에 직접 관계된 부분만: `TopNav` 로 상단 바, `Stack`/`VStack` 으로 레이아웃, `Button` 으로 다운로드 둘, `Text` 로 타이포. 테마는 `data-astryx-media`. **CDN 없이 벤더링**하므로 GitHub Pages 정적 호스팅에 그대로 올라간다.

랜딩이 Astryx 를 쓰는 것은 예쁘게 하려는 것이 아니라 **랜딩·PWA·데스크톱이 같은 시각 언어를 갖게** 하려는 것이다. 사용자가 랜딩에서 본 것과 앱에서 보는 것이 같아야 한다.

## 완료 판정

- 4 개 언어가 다 뜬다
- 다운로드 링크가 릴리즈에서 파생되고 손으로 적힌 버전이 없다
- `landingBudget` · `landingLinksAlive` · `frontendVendored` · `themeContract` 가 CI 에서 돈다
- 라이트·다크 둘 다 확인됨
