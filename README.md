# Naru

Naru는 macOS용 Rust, CEF 브라우저 시제품입니다. CEF에 포함된 Chromium으로 웹페이지를 표시하고, Rust CLI가 실행 중인 앱에 로컬 소켓으로 명령을 전달합니다. Electron, Node 런타임, MCP 서버는 사용하지 않습니다.

브라우저 창 하나 안에서 내 공간과 작업별 AI 공간을 전환합니다. 접을 수 있는 세로 탭, 공간 카드와 미리보기, 주소창, 작업 상태와 중지 버튼은 네이티브 UI입니다. CLI 제어를 유지하며 내장 AI 채팅과 모델 API 연결은 아직 구현하지 않았습니다. 탭 행마다 닫기 버튼을 제공하고 같은 탭·공간의 AppKit 컨트롤을 재사용합니다. 탭 행 v1은 빈 탭의 선택·주소 초안 보존과 AI 인계를 확인했고, 사용자 승인 후 같은 바이너리의 로컬 HTTP·이탈 확인 취소/확정과 CEF 회귀 32개를 통과했습니다. 후속 검색 별칭 빌드도 네이티브 검색·로컬 HTTP·이탈 확인·CEF 회귀 32개를 통과했고, 같은 바이너리와 프로필로 정상 재시작한 뒤 추가 승인 질문 없이 HTTP 완료를 확인했습니다.

개발 목표는 Naru 자체를 사람과 AI가 함께 쓰는 독립 브라우저로 완성한 뒤, 같은 Rust·CEF 구현을 Preseller 화면 안에 내장하는 것입니다. 현재는 독립 앱 단계이며 공통 코어 추출과 내장 연결은 구현 전입니다. [독립 완성과 내장 계획](docs/preseller-integration.md)에 단계와 완료 기준을 기록합니다.

[제품 완성 계획](docs/product-plan.md)은 Aside의 일상 탐색 사용성과 ego lite의 사용자·AI 작업 공간 및 외부 제어를 결합하는 개발 순서를 추적합니다. 현재 우선순위는 세로 탭·검색·북마크 등 일상 브라우저 사용성입니다.

CLI의 [Enter 입력](docs/keyboard-input.md)을 Chromium 키 요청으로 바꾸는 소스 구현을 추가했습니다. 대상 포커스 확인 뒤 키 누름과 뗌을 보내며 버튼, 줄바꿈과 제출의 기본 동작은 엔진이 처리합니다. 실제 Naru 앱의 통합 검증과 기존 앱 번들 반영은 남아 있습니다.

페이지 우클릭 메뉴에 [제어권 재검사](docs/context-menu-control.md)를 소스로 추가했습니다. 메뉴를 연 뒤 AI가 재개되거나 탭, 문서와 프레임이 바뀌면 이전 명령을 거절합니다. 기본 메뉴는 유지하며 실제 네이티브 동작 검증은 남아 있습니다.

HTTP(S) 링크에는 **링크 주소를 새 탭에서 열기** 항목을 추가했습니다. 같은 공간의 배경 탭에 주소를 열어 현재 페이지를 유지합니다. 원래 링크의 JavaScript나 referrer를 재현하는 기능은 아니며, 기존 앱 번들 반영과 실제 메뉴 검증은 남아 있습니다.

파일 메뉴의 [현재 주소를 내 공간 새 탭에서 열기](docs/open-current-page.md)를 소스에 추가했습니다. AI 원본 탭의 소유권을 유지하면서 실제 페이지 주소를 새 사용자 탭에서 여는 기능입니다. 기존 앱 번들에는 아직 포함하지 않았으며 네이티브 메뉴와 실제 탐색 검증은 남아 있습니다. 폼 입력이나 문서 상태를 복제하는 기능은 아닙니다.

보기 메뉴에 [현재 탭 음소거](docs/tab-audio.md)를 소스로 추가했습니다. 현재 표시한 탭의 출력 설정만 바꾸며 AI 제어권과 주소 초안을 유지합니다. 체크와 탭 아이콘은 확인된 음소거 설정을 나타내며 재생 중인 소리 감지는 지원하지 않습니다. 기존 앱 번들 반영과 실제 소리 검증은 남아 있습니다.

보기 메뉴의 **현재 탭 위로 이동·아래로 이동**과 사이드바 드래그 정렬을 소스로 추가했습니다. 긴 목록에서는 가장자리에 포인터를 두면 자동으로 스크롤합니다. 같은 공간 안에서 순서만 바꾸며 선택과 AI 제어권을 유지합니다. 번호 선택과 탭 순환도 바뀐 표시 순서를 따릅니다. 실행 중 순서만 보관하고 기존 앱에는 아직 반영하지 않았습니다. [동작과 검증 경계](docs/tab-order.md)를 참고하세요.

탭 순환의 입력 보호도 소스에서 보완했습니다. 기존 `Ctrl+Tab`, `Ctrl+Shift+Tab`, `⌘⇧[`와 `⌘⇧]`에 더해 `보기 → 이전 탭`, `보기 → 다음 탭`으로 현재 공간의 순서대로 이동합니다. 한 탭뿐이면 검색 화면과 입력을 유지하며, 실제 탭 선택이 실패해도 먼저 화면을 닫거나 초안을 지우지 않습니다. 번호 선택과 순환은 입력기 조합 중에 실행하지 않습니다. 기존 앱 번들은 바꾸지 않았고 네이티브 입력 검증은 남아 있습니다.

새 탭에는 중앙 주소·Google 검색 입력과 북마크 진입을 제공하는 네이티브 시작 화면을 추가했습니다. 앱 시작과 명시적인 새 탭에만 표시하며, 웹사이트의 `about:blank` 팝업이나 CLI가 만든 문서를 덮지 않습니다. 같은 탭의 배경 작업·오버레이 왕복은 입력을 보존하고 탭·공간·탐색 세대가 바뀌면 지웁니다. [시작 화면 설계](docs/start-page-design.md)에 표시·포커스·AI 인계 경계를 기록했습니다. 최종 start-page-v2는 Rust 194개와 로컬 CEF 회귀 32개를 통과했으며, 중앙 입력·팝업 입력·초안 보존·AI별 인계를 실제 네이티브 화면에서 확인했습니다. 바이너리별 검증과 남은 항목은 [작업 기록](docs/work-log.md)을 기준으로 합니다.

후속 ui-focus-v3에서는 편집 중 사이드바 북마크·검색·다운로드를 열면 중앙 초안이 사라지는 문제를 보완해 실제 왕복 보존을 확인했습니다. Rust 194개·전체 타깃 검사·release 빌드와 같은 최종 바이너리의 로컬 CEF 회귀 32개가 통과했습니다. 키체인 승인 후 HTTP·팝업 입력·AI별 인계와 검증 자원 정리도 확인했습니다. 해당 빌드에서는 주소창→중앙 입력 Tab 이동과 사이드바 펼침 직후 모서리 표시가 후속 보완 항목으로 남았습니다.

이후 native-focus-v1 진단에서 숨긴 Chromium 자식이 유효한 키보드 이동 후보와 실제 first responder가 되는 것을 확인했습니다. native-focus-v2는 AppKit의 원래 Tab 처리가 끝난 뒤에도 숨긴 CEF에 포커스가 남은 경우만 다음 유효 후보로 이동합니다. Rust 194개, 전체 타깃 검사, format, release 빌드가 통과했습니다. 진단을 켠 실행에서 2방향, 끈 실행에서 주소창과 중앙 입력 사이 Tab, ShiftTab 4방향 이동을 확인했습니다. 이 중간 빌드에서는 HTTP와 로컬 CEF 회귀를 실행하지 않았습니다. 근거는 `artifacts/native-focus-v2-native.json`이며 전체 버튼 접근성 검증과 구분합니다.

최종 native-focus-v3는 Rust 194개, 전체 타깃 검사, format, release 빌드와 같은 바이너리의 로컬 CEF 회귀 32개를 통과했습니다. 진단을 끈 상태에서 주소창과 중앙 입력 사이 Tab, ShiftTab 네 방향, 중앙 실제 입력과 북마크·검색·다운로드 왕복의 초안 보존을 확인했습니다. 배경 AI의 HTTP 탐색은 현재 선택과 초안을 보존했고, AI 공간의 Tab과 입력은 제어권을 유지했습니다. 유효한 HTTP 주소를 Enter로 제출할 때만 해당 AI가 인계됐습니다. 웹 본문의 Tab과 Return, 내용이 있는 `about:blank` 팝업 입력도 확인했습니다. 추가 키체인 질문 없이 HTTP가 완료됐으나 빌드 사이의 권한 유지가 해결됐다는 뜻은 아닙니다. 자체 검증 작업과 서버를 종료하고 원래 사용자 탭 하나와 최종 앱을 남겼습니다. 근거는 `artifacts/native-focus-v3-native.json`, `artifacts/native-focus-v3-suite.json`, `artifacts/native-focus-v3-suite-state.json`입니다.

사이드바 모서리 잘림은 원인과 재현 조건이 미확정입니다. native-focus-v1에서 다시 발생했고, v2도 좌표 진단을 켰을 때만 정상으로 보였습니다. v3의 조건부 `displayIfNeeded` 적용 뒤 초기 검사에서도 잘림이 재현됐습니다. 이후 같은 실행의 접기·펼치기 두 차례와 같은 바이너리 재실행, 독립 캡처에서는 정상이었지만 초기 관찰을 취소하거나 해결로 판정하지 않습니다.

후속 `surface-probe-v1`은 getter별 영향을 비교할 진단만 추가했고 format과 release 빌드를 통과했습니다. 기본 모드의 표시와 진단 로그 0건, `noop` 대조군의 토글 18회와 로그 상한 16회를 확인했습니다. 마지막 화면은 정상이었으며 매회 이미지를 판독한 검사는 아닙니다. 잘림을 재현한 대조군이 없어 getter 실험과 효과 판정은 하지 않았습니다. 기본 모드에서 새 탭의 Tab 입력, 북마크 왕복 초안 보존, 중앙 HTTP 제출과 CLI 읽기를 확인했습니다. 추가 Rust 검사, 전체 CEF 32개 회귀와 모의 DOM 검사는 실행하지 않았으며 앞선 빌드의 194개와 32개 결과를 합산하지 않습니다. 첫 종료는 watchdog exit 2였고 이후 두 종료는 exit 0으로, 원인은 미확정입니다. 진단은 기본 OFF이고 새 캐시, 타이머, 웹 UI는 없습니다. FKA 활성 설정, 실제 IME, 최소 창, 다크 모드, 외부 서비스와 RAM 비교는 미검증입니다. 상세 근거와 정리는 [시작 화면 설계](docs/start-page-design.md)와 [작업 기록](docs/work-log.md)에 남깁니다.

Aside의 탐색 구성과 토스의 공개 디자인 원칙을 참고한 **1차 UI를 적용하고 최종 v2에서 검증했습니다**. 사이드바 상단에 검색·공간 전환을 두고 세로 탭은 38pt 간격으로 배치했습니다. 하단 북마크·다운로드는 보조 행동으로, 40pt 파란 새 탭 버튼은 주요 행동으로 구분했습니다. 중립색 표면과 파란 강조색, 시스템 폰트, 56pt 도구 모음과 36pt 주소 영역으로 화면 위계를 조정했습니다. 기존 네이티브 뷰·사용자/AI 공간·CLI 계약을 유지합니다.

최종 aside-toss-v2는 Rust 190개·전체 타깃·format·release와 로컬 CEF 회귀 32개를 통과했습니다. 네이티브 화면에서 새 탭의 아이콘·글자 정렬과 주소 포커스, 배경 탭을 닫은 뒤 주소 초안 이어 쓰기, 한글 붙여넣기 검색, 긴 제목 말줄임, 접힌 탭 행 닫기와 빈 AI 공간의 개별 중지를 확인했습니다. 원래 사용자 탭 1을 보존하고 이전 앱·자체 서버·대기 자원을 정리했습니다. 근거는 `artifacts/aside-toss-v2-native.json`, `artifacts/aside-toss-v2-suite.json`, `artifacts/aside-toss-v2-validation.json`입니다. 첫 HTTP는 키체인 API 대기와 함께 시간 초과였으나 같은 바이너리에서 이후 완료됐습니다. 최종 v2의 실제 최소 창·다크 모드·IME 조합·전체 키보드 이동·RAM 절감·재빌드 간 키체인 권한 유지는 미검증이며, 이 결과는 전체 디자인이나 브라우저·Preseller 내장의 완성을 뜻하지 않습니다.

[설계 문서](docs/architecture.md)에서 전체 구조, 개발 단계, 공간과 작업 소유권의 CLI 계약을 설명합니다. 표시할 공간과 탭의 선택 상태는 작업 제어권과 따로 관리합니다. 모든 공간은 같은 브라우저 프로필을 공유합니다.

저장소와 프로젝트 이름은 Naru입니다. 현재 검증된 시제품의 앱 번들은 `Light Browser.app`, CLI는 `lightbrowser`이며, 프로필과 키체인 식별자도 기존 이름을 사용합니다.

## 빌드와 실행

필요한 도구는 Rust, Cargo, Xcode Command Line Tools, CMake, Python 3입니다. macOS ARM64에서 실행을 검증했습니다. Intel용 배포 구성은 준비돼 있지만 실제 실행은 확인하지 않았습니다.

```bash
git clone https://github.com/FEABE/naru-browser.git
cd naru-browser
./scripts/dev.sh --release --build-only
```

최초 빌드 시 고정된 CEF 156.0.1 beta 런타임을 `.cache/cef`에 내려받습니다. Ninja가 설치돼 있지 않으면 검증한 공식 바이너리를 `.cache/bin`에 받습니다. CEF는 공식 배포 인덱스의 파일 크기와 SHA-1, Ninja는 고정한 공식 SHA-256을 확인합니다. 전역 설치는 하지 않습니다. Cargo는 기본 Cargo 캐시를 사용합니다.

생성된 앱은 `dist/Light Browser.app`입니다. 기본 동작은 앱을 만드는 것까지이며 자동 실행하지 않습니다. `--build-only`는 이 동작을 명시하는 옵션입니다. 서명 기본값은 개발용 ad hoc이며 배포용 공증은 하지 않았습니다.

새 앱의 서명 검증이 끝나면 이 프로젝트 경로에서 실행 중인 이전 앱을 정상 종료하고 교체합니다. 종료되지 않으면 기존 앱을 보존한 채 빌드를 중단합니다. 교체 후에는 이전 앱 번들과 Cargo 중간 산출물을 삭제하고 선택한 빌드 모드의 실행 파일 3개만 남깁니다. 따라서 다음 빌드는 재컴파일 시간이 늘어날 수 있습니다. 현재 CEF 런타임 캐시와 실제 브라우저 프로필은 유지합니다. 자동 빌드 정리는 이 저장소의 `target`에만 적용하며 외부 `CARGO_TARGET_DIR`은 거부합니다.

**키체인 처리:** CEF 156의 앱 전용 키체인 API를 적용했습니다. `Light Browser Safe Storage` / `local.lightbrowser.dev` 이름을 사용하며 다른 Chromium 앱의 키를 읽지 않습니다. 이전 시제품의 `profile`은 그대로 두고 새 `profile-v1`을 사용하므로 기존 로그인 데이터를 자동 이전하지 않습니다.

Chromium 로그는 `~/Library/Logs/Light Browser/chromium.log`에 저장합니다. Documents 안의 로그 파일을 여는 단계에서 GPU 보조 프로세스가 대기하는 현상이 확인돼, 프로필 위치와 로그 위치를 분리했습니다.

현재 번들의 ad hoc 서명은 코드 해시로 앱을 식별합니다. 변경된 빌드를 실행하면 키체인 접근 승인이 반복될 수 있습니다. 이를 매번 입력하는 개발 흐름을 중단하기 위해 빌드와 실행을 분리했습니다. 키체인 권한 유지 문제 자체가 해결된 것은 아니며, 최종 numbered-tabs-v3의 HTTP와 CEF 검증은 미완료입니다.

빌드 스크립트는 선택한 코드 서명 인증서를 `NARU_CODESIGN_IDENTITY`로 지정할 수 있습니다. 직접 번들 스크립트를 실행할 때는 `--signing-identity`를 사용합니다. 지정하지 않으면 기존 ad hoc 서명을 사용하고, 명시한 값이 비었거나 서명에 실패하면 중단합니다. 다른 서명으로 자동 대체하지 않습니다. 이 옵션은 인증서를 만들거나 키체인 권한을 변경하지 않으며 모든 중첩 코드와 앱에 같은 identity를 전달합니다. 실제 인증서를 지정한 두 빌드의 권한 유지 검증은 아직 하지 않았습니다.

```bash
NARU_CODESIGN_IDENTITY='선택한 코드 서명 인증서의 이름 또는 해시' ./scripts/dev.sh --release --build-only
```

이미 준비한 서명자를 선택하고 앱 실행까지 명시하려면 `--run`을 사용합니다. 이때 서명자 미지정, 빈 값, ad hoc 값 `-`는 다운로드나 컴파일 전에 거절합니다. `--run`과 `--build-only`를 함께 지정할 수 없으며, 브라우저 인수는 `--run --` 뒤에만 전달합니다.

```bash
NARU_CODESIGN_IDENTITY='선택한 코드 서명 인증서의 이름 또는 해시' ./scripts/dev.sh --release --run
```

서명자를 지정하는 것만으로 최초 승인이나 변경 빌드 사이의 권한 유지가 보장되지는 않습니다. 인증서 생성, 선택, 키체인 설정 변경은 자동화하지 않습니다. 현재 검증은 임시 checkout과 가짜 빌드 도구를 사용한 스크립트 검사이며 실제 인증서 서명이나 앱 재실행 검사가 아닙니다.


## CLI

`scripts/dev.sh`는 브라우저 데이터를 프로젝트의 `.local-data`에 저장합니다. 다른 터미널의 CLI에도 같은 경로를 설정합니다.


```bash
export LIGHT_BROWSER_DATA_DIR="$PWD/.local-data"
./target/release/lightbrowser status
./target/release/lightbrowser open https://www.naver.com
./target/release/lightbrowser tabs
```

`open`은 내 공간에 새 탭을 만든 직후 반환합니다. 내 공간을 보고 있을 때만 새 탭을 선택하며 다른 AI 공간을 보고 있으면 현재 화면을 유지합니다. 공간 목록 화면에서도 기존 선택을 바꾸지 않습니다. 내 공간에 선택이 없을 때는 첫 탭을 대표 탭으로 기록합니다. 로딩 완료를 뜻하지 않으므로 응답의 `result.tab`을 이후 명령에 사용합니다. 아래 `3`은 예시입니다.

```bash
./target/release/lightbrowser wait 3 dom 10000
./target/release/lightbrowser read 3
./target/release/lightbrowser scroll 3 0 600
./target/release/lightbrowser fill 3 'read에서 받은 ref' '가벼운 브라우저'
./target/release/lightbrowser press 3 '같은 ref' Enter
./target/release/lightbrowser read 3
./target/release/lightbrowser screenshot 3 "$PWD/artifacts/example.png"
./target/release/lightbrowser close 3
```

`fill`은 input, textarea와 명시적인 HTML `contenteditable` 영역에 텍스트를 넣습니다. 일반 입력칸은 기존 input/change 이벤트 뒤 실제 값이 요청한 문자열과 같은지 확인합니다. 브라우저의 정규화나 페이지 처리로 값이 달라지면 오류를 반환하며 자동 재시도하지 않습니다. 편집 영역은 전체 내용을 일반 텍스트로 교체하며 서식과 undo를 보존하지 않습니다. [일반 입력칸 계약](docs/native-fill-control.md)과 [편집 영역 입력 계약](docs/contenteditable-control.md)을 참고하세요.

같은 탭에서 주소를 변경하거나 방문 기록을 이동할 수 있습니다. 탐색 명령은 요청 수락을 반환하므로 이후 `wait`로 준비 상태를 확인합니다.

```bash
./target/release/lightbrowser navigate 3 https://example.com
./target/release/lightbrowser wait 3 load 10000
./target/release/lightbrowser back 3
./target/release/lightbrowser wait 3 load 10000
./target/release/lightbrowser forward 3
./target/release/lightbrowser wait 3 load 10000
./target/release/lightbrowser reload 3
./target/release/lightbrowser stop 3
```

`tabs`의 `canGoBack`과 `canGoForward`로 방문 기록 이동 가능 여부를 확인합니다. 기록이 없으면 `NO_HISTORY`, 아직 로딩 중이면 새 탐색 명령은 `NAVIGATION_BUSY`를 반환합니다. 중단하려면 `stop`을 호출하고 `tabs`의 `loading:false`를 확인한 뒤 새 주소로 이동합니다. 중지된 탐색의 `wait`는 `NAVIGATION_STOPPED`를 반환하며, 새 탐색을 시작하면 중지 상태가 초기화됩니다.

`scroll TAB DX DY`는 상대 이동입니다. 각 축은 -10,000~10,000 CSS 픽셀 정수이며 결과의 `page.x`, `page.y`는 실제 이동 후 절대 위치입니다. `read.page.viewport`는 현재 스크롤 위치와 화면 크기를 반환합니다. 본문 읽기의 12,000자 제한은 스크롤 위치와 무관하게 문서 처음부터 적용합니다.

`wait TAB [dom|load] [TIMEOUT_MS]`의 기본값은 `dom`, 10,000ms이며 1~30,000ms를 허용합니다. `dom`은 현재 문서의 `readyState`가 `interactive` 또는 `complete`인 상태, 즉 DOM 파싱 완료를 뜻합니다. DOMContentLoaded 이벤트나 웹앱의 비동기 데이터 준비까지 보장하지 않습니다. `load`는 메인 문서가 존재하고 브라우저 전체 로딩이 끝난 상태를 기다립니다.

`read`는 메인 문서가 생성된 뒤부터 가능합니다. `page.readyState`가 `loading`이면 부분 본문일 수 있습니다. 이미지·하위 프레임 로딩이 남아 있어도 읽을 수 있으며, 준비 상태 확인에는 요소 참조를 만료시키지 않는 `wait`를 사용할 수 있습니다.

`WAIT_TIMEOUT`은 탭을 닫거나 탐색을 취소하지 않습니다. 오류의 `details`에 탭 ID, 경과 시간, 당시 상태가 포함됩니다. `tabs`는 요청 URL, commit 여부, 문서 존재, 메인 로드 완료, Chromium 탐색 오류를 반환합니다. `status.pendingWaits`로 대기 요청 수를 확인할 수 있습니다.

`tabs`의 `renderer.status`는 `running`, `terminated`, `recovering`으로 렌더러 종료와 복구 상태를 구분합니다. 예상치 못한 렌더러 종료는 해당 탭에 남은 요청과 대기를 `RENDERER_TERMINATED`로 끝내고 기존 요소 참조를 만료시킵니다. 탭과 작업 소속은 유지하며, 이미 페이지에 전달된 변경 작업만 `outcomeUnknown:true`로 표시합니다. 자동으로 조작을 재시도하지 않습니다. `reload` 또는 `navigate`로 복구를 요청하면 `recovering`이 되고 새 문서가 commit된 뒤 `running`으로 돌아옵니다. 이 값은 페이지 준비 완료를 뜻하지 않으므로 이후 `wait`와 `read`로 확인합니다.

204 응답처럼 새 문서도 같은 문서 내 이동도 완료되지 않은 탐색은 `NAVIGATION_INCOMPLETE` 또는 Chromium의 `NAVIGATION_FAILED`로 보고합니다. 이전 본문이 읽힌다는 이유로 새 탐색이 성공한 것으로 처리하지 않습니다.

스크린샷의 상위 폴더는 미리 만들어야 하며 기존 파일을 덮어쓰지 않습니다. 모든 명령은 stdout에 JSON 하나를 반환합니다. 오류는 stderr에도 표시하며 0이 아닌 종료 코드를 사용합니다. `help`에서 명령 목록을 확인할 수 있습니다. CLI는 `.app/Contents/MacOS/lightbrowser`에도 포함됩니다.

앱을 Finder에서 직접 열면 기본 프로필은 `~/Library/Application Support/Light Browser`입니다. 이때 CLI에서도 `LIGHT_BROWSER_DATA_DIR`을 해제하거나 같은 프로필 경로를 지정합니다. 다른 브라우저의 프로필과 로그인은 가져오지 않습니다.

## 폼 선택 명령

`check TAB REF true|false`를 소스에 추가했습니다. 기본 HTML 체크박스의 선택과 해제, 라디오 선택을 지원하며 이미 원하는 상태면 클릭하지 않습니다. 한 번 클릭한 뒤 상태를 확인하고 취소나 요소 교체를 오류로 구분합니다. 클릭 시도 여부도 CLI 오류 상세에 남깁니다. 모의 DOM과 독립 ego lite Chromium 검사를 통과했으며 기존 Naru 앱 번들은 아직 교체하지 않았습니다. [체크 제어 계약과 검증 범위](docs/check-control.md)를 참고하세요.

`select TAB REF VALUE`와 작업 범위의 같은 명령을 앱·release CLI에 반영했습니다. 기본 HTML 단일 선택만 지원하며 옵션의 정확한 값을 사용합니다. 빈 값·공백을 보존하고 read 옵션 수와 선택 검사 수에 상한을 둡니다. 키체인 승인 후 실제 화면의 선택값·합성 이벤트·동일값 무동작과 직접 클릭 시 AI 작업 중지를 확인했습니다. 최초 자동 검사의 배경 focus 가정을 독립 진단으로 바로잡은 뒤 같은 바이너리에서 선택 10개와 기존 회귀 22개가 통과했습니다. 후속 shortcut v1에서는 검색·사이드바·공간 보기 단축키가 AI 작업을 유지하고, 직접 클릭·붙여넣기는 정상적으로 AI를 중지하는 동작까지 확인했습니다. 같은 최종 바이너리의 회귀 32개와 재시작 후 로컬 HTTP도 통과했습니다. [선택 제어 계약](docs/select-control.md)을 참고하세요.

## 주소·검색·북마크

후속 소스는 통합 검색 결과 아래에 ‘주소 열기’ 또는 ‘Google에서 검색’ 행을 제공합니다. 기존 탭·북마크·기록을 우선하며 명시적으로 새 행을 선택했을 때만 내 공간에서 엽니다. 입력 중 외부 요청은 없고, 200자를 넘어 잘린 입력은 수정 전까지 새 탐색하지 않습니다. [검색 탐색 계약](docs/search-navigation.md)에 실패·AI 공간·검증 경계를 기록했습니다. 실제 Naru 앱 연결 검증은 남아 있습니다.

방문 기록은 **소스 구현과 단위 검사 단계**입니다. 내 공간의 소유되지 않은 탭에서 확인된 HTTP(S) URL, 제목, 최근 방문 시각만 `history-v1.json`에 저장하며 최근 30일, 최대 1,000개와 2MiB로 제한합니다. 과거 목록 백업은 만들지 않습니다. 통합 검색의 방문 기록 결과와 `보기 → 방문 기록`, 검색어를 유지하는 전용 보기, 확인 후 전체 삭제를 연결했습니다. 한 파일 작업과 최대 64개, 256KiB의 합쳐진 대기를 사용하며 별도 웹 UI나 상주 프로세스는 없습니다. 삭제 당시 열린 문서는 다음 전체 탐색 전까지 같은 문서 이동도 기록하지 않습니다. [방문 기록 설계와 검증 범위](docs/history-design.md)에 기록했습니다. 기존 앱 바이너리는 교체하지 않았으며 실제 CEF 수집, 화면, IME와 메모리는 아직 검증하지 않았습니다.

숫자 탭 단축키 `⌘1`~`⌘8`은 현재 표시 공간의 사이드바 순서로 탭을 선택하고 `⌘9`는 마지막 탭을 선택합니다. 없는 번호이거나, 목록 화면과 공간 둘러보기가 닫힌 상태에서 같은 탭을 선택하면 현재 입력을 유지합니다. 실제 탭 선택에 성공한 경우에만 열린 목록 화면을 닫으며 AI 작업의 제어권은 유지합니다. 새 캐시나 웹 renderer는 추가하지 않았습니다. Rust 198개·전체 타깃·format·release와 최종 앱의 숫자 선택·입력 보존·AI 상태 보존을 확인했습니다. 검색·북마크 왕복에서 상단 주소 초안이 사라지던 누락도 보완했습니다. HTTP·로컬 CEF 검증은 키체인 대기로 완료하지 못했고 사용자의 추가 승인 요청 중단 지시에 따라 검사 앱·작업·탭·서버를 정리했습니다. [작업 기록](docs/work-log.md)에 실행 범위를 구분했습니다.

새 탭은 주소 입력에 포커스를 둡니다. 주소창은 HTTP(S) 주소와 일반 검색어를 구분하며 일반 검색어는 Google 검색 주소로 변환합니다. 잘못된 주소나 지원하지 않는 스킴은 오류로 표시합니다. 사용자 이름이나 비밀번호가 포함된 주소를 검색 서비스로 보내지 않도록 검사합니다.

사이드바의 검색 버튼이나 `⌘⇧A`에 열린 탭과 북마크를 함께 표시합니다. 표시 제목인 “새 탭”도 원래 제목·URL·공간과 함께 검색합니다. 독립 검색 검사 6개와 전체 Rust 검사 179개·전체 타깃 Cargo 검사를 통과한 소스를 tab-search-label-v1 release 앱에 반영했습니다. 실제 검색창에 “새”와 “새 탭”을 각각 붙여넣었을 때 원래 사용자 빈 탭 하나가 결과로 표시됐고 Return으로 탭 1을 선택했습니다. IME 조합 검사는 아닙니다. 첫 HTTP의 commit 전 WAIT_TIMEOUT은 `artifacts/tab-search-label-v1-validation.json`에 보존했습니다. 사용자 승인 후 같은 최종 바이너리에서 HTTP 완료, 배경 행 닫기의 이탈 확인 취소·확정과 선택·주소 초안·재포커스 없는 이어쓰기, CEF 회귀 32개를 확인했습니다. 정상 종료와 같은 바이너리·프로필의 재시작 뒤 추가 승인 질문 없이 HTTP가 완료됐으며 검사 탭과 서버를 정리해 원래 사용자 빈 탭 1과 대기 0개만 남겼습니다. 근거는 `artifacts/tab-search-label-v1-final-validation.json`입니다. 다른 빌드 사이 키체인 권한 유지 검증은 아닙니다. 탭은 제목·URL·공간 이름, 북마크는 제목·URL을 검색하고 탭 결과를 먼저 둡니다. 입력 최대 200자, 전체 결과 최대 40개이며 이때 검증한 앱에는 방문 기록이 포함되지 않았습니다. 현재 방문 기록 소스의 검증 범위는 위 문단과 구분합니다. 방향키·Enter 또는 마우스로 열고 Esc로 닫습니다. v2의 실제 로컬 페이지에서 탭·북마크 선택, 사용자 탭 재사용과 새 탭 열기, AI 작업의 상태·revision·선택 보존을 확인했습니다. 검색 중 현재 탭을 닫으면 입력 포커스를 잃는 결함은 v3에서 수정했습니다. 빈 탭과 HTTP 문서의 검색 입력, 북마크 편집 중 원래 탭을 닫아도 입력 포커스와 고정한 URL을 유지하는 네이티브 회귀를 통과했습니다.

사이드바 북마크 버튼과 `⌘D`로 네이티브 북마크 화면을 엽니다. `⌘D`는 현재 페이지의 URL을 고정한 이름 편집 화면을 열고, 이미 저장한 URL이면 기존 이름을 편집합니다. 목록에서 열기·이름 수정·삭제와 제목·URL 검색을 제공합니다. 별도 북마크 화면은 입력 최대 200자, 결과 최대 30개입니다. 방향키로 선택하고 Enter로 열며 Esc로 편집이나 목록을 닫습니다.

북마크 열기는 내 공간의 같은 URL을 가진 소유권 없는 탭을 먼저 사용하고, 없으면 내 공간에 새 탭을 만듭니다. 사용자가 요청한 열기이므로 내 공간으로 전환하지만 AI 작업의 제어권과 저장된 탭 선택은 유지합니다. 파일 읽기와 저장은 한 번에 한 건을 짧은 작업 스레드에서 처리하며 정상 종료 전에 진행 중인 저장을 기다립니다. 모든 공간은 같은 데이터 폴더의 목록을 공유합니다. 승인한 최대 1,000개·2MiB, 백업·손상 보호와 구현 경계는 [북마크 설계](docs/bookmarks-design.md)에 있습니다.

북마크 v1 앱에서 추가·이름 수정·삭제·검색·탭 재사용, 정상 종료 후 두 항목 보존과 마지막 항목 삭제 후 빈 목록 유지를 확인했습니다. 직접 만든 손상 프로필에서는 읽기 전용 표시·변경 차단·정상 백업 항목 열기와 원본 파일 보존을 확인했습니다. 이는 로컬 페이지를 사용한 검증입니다.

v2에는 초기·미완료 문서의 공간 미리보기 캡처를 시작하지 않는 보호도 포함했습니다. 사용자 승인 후 같은 바이너리에서 로컬 CEF 회귀 22개가 통과했습니다. 응답 헤더나 이미지 로딩을 기다리는 카드는 안내 화면을 유지하고, 로딩이 끝나면 공간 목록을 다시 열지 않아도 실제 미리보기로 바뀌는 것을 확인했습니다. 이는 로컬 동작 검증이며 메모리 절감 측정은 아닙니다.

## 공간 전환과 작업 제어

브라우저 UI는 옅은 중성 배경, 파란색 선택 표시, 상단 검색과 여백을 둔 세로 탭, 시스템 아이콘을 사용합니다. 주소창과 작업 상태를 분리하고 공간 카드 아래에 제목·탭 정보·상태·중지를 모았습니다. 좁은 창에서는 카드를 한 열로 표시합니다. UI는 AppKit이며 별도 웹 renderer나 반복 갱신 타이머는 추가하지 않았습니다. 2026-10-13 UI 변경·검증·남은 제약은 [작업 기록](docs/work-log.md)에 있습니다.

`user`는 사용자의 내 공간이며 작업 공간의 ID는 `task create`가 반환한 작업 ID와 같습니다. 각 공간은 자신의 탭 목록과 마지막 선택을 기억합니다. 공간 분리는 화면 구성과 탭 소유권을 위한 기능이며 쿠키, 로그인 상태, 브라우저 프로필을 격리하지 않습니다.

```text
lightbrowser spaces
lightbrowser space overview
lightbrowser space show user
lightbrowser space show ID
```

`spaces`는 `activeSpace`, `activeTab`, `overview`와 공간 목록을 반환합니다. 각 공간의 `tabs`는 탭 ID 목록이며 `selectedTab`은 그 공간의 선택입니다. 탭이 없으면 `null`입니다. `status`에도 현재 공간과 overview 상태가 있고 `tabs`의 `space`는 표시 공간, `task`는 CLI 소유권을 나타냅니다. 공간 목록을 열어도 저장된 선택은 유지하지만 페이지 뷰는 숨깁니다.

`space show`는 해당 공간과 저장된 탭을 표시하고 `space overview`는 공간 목록을 엽니다. 두 명령은 브라우저 창을 앞으로 가져오며 작업 중지나 제어권 인계를 수행하지 않습니다. `task run ID REV spaces`는 그 작업의 공간 항목만 반환합니다. 작업 범위에서 `space show`나 `space overview`를 호출하면 `SPACE_UI_REQUIRES_UNSCOPED`로 거부합니다.

`task create NAME`으로 작업을 만들고 응답의 `result.task.id`와 `revision`을 이후 명령에 사용합니다. 새 작업은 탭이 없는 `active` 상태이며 revision은 1입니다. 빈 공간을 만들 때 CEF 탭을 추가하지 않습니다. 작업 생성과 작업 범위의 `open`은 현재 공간과 기존 선택을 바꾸지 않습니다. 선택이 없던 공간은 첫 탭을 대표 탭으로 기록하며 이후 추가 탭은 선택하지 않습니다. 각 CLI가 종료돼도 같은 앱이 실행 중이면 작업은 유지됩니다.

```text
lightbrowser task create "자료 조사"
lightbrowser task list
lightbrowser task info ID
lightbrowser task run ID REV open https://example.com
lightbrowser task run ID REV tabs
lightbrowser task run ID REV read TAB
lightbrowser task adopt ID REV TAB
lightbrowser task release ID REV TAB
lightbrowser task handoff ID REV
lightbrowser task resume ID REV
lightbrowser task finish ID REV --keep TAB
```

위 `ID`, `REV`, `TAB`은 응답에서 받은 값으로 바꿉니다. `handoff`, `resume`, 최초 `finish`는 revision을 증가시키므로 이후 명령에는 새 값을 사용합니다. `handoff`는 대기 중인 요청을 중단하고 이전 요소 참조를 만료시킵니다. 이미 전달한 조작은 되돌리지 않으며, 해당 요청에 `outcomeUnknown:true`를 반환할 수 있습니다. 재개 후에는 페이지를 다시 읽습니다.

작업이 소유한 탭은 `task run`으로 접근합니다. 다른 작업이나 일반 명령으로 접근하면 거부합니다. 기존 탭을 `adopt`하면 빌린 탭으로 기록하고 해당 작업의 표시 공간으로 옮깁니다. `release`는 소유권을 해제하고 빌린 탭을 원래 공간으로 돌려보냅니다. 작업이 직접 만든 탭을 release하면 내 공간으로 옮깁니다.

`finish`는 빌린 탭을 원래 공간으로 돌려보내고 `--keep`에 지정한 created 탭은 완료된 작업 공간에 남깁니다. 두 경우 모두 CLI 소유권은 해제합니다. 나머지 created 탭만 정상 닫기를 요청합니다. 응답의 `closeRequested`는 닫기 완료가 아니며 `task info`가 `finished`인지 확인해야 합니다. 브라우저 확인 창 때문에 닫기가 지연되면 `finishing` 상태를 유지합니다. 탭이 없는 완료 공간은 목록에서 제거하지만 다른 작업에 빌려준 탭의 복귀 목적지라면 보존합니다.

브라우저의 상태 표시와 공간 카드에서 작업 상태를 확인하고 active 작업을 중지할 수 있습니다. 중지는 해당 작업에 handoff를 요청해 이전 CLI revision과 요소 참조를 만료시킵니다. 다른 작업과 열린 페이지는 유지합니다. 재개는 기존 `task resume` CLI로 요청합니다. `lightbrowser panel`은 기존 명령 호환을 위해 브라우저 창을 앞으로 가져옵니다.

주소창, 뒤로, 앞으로, 새로고침, 탭 닫기로 작업 소유 탭을 조작하거나 active 공간에서 새 탭을 만들면 먼저 해당 작업을 handoff합니다. 페이지 본문 클릭, 키 입력, 휠과 편집 메뉴도 같은 경로를 사용합니다. v6의 로컬 네이티브 검사에서 본문 클릭·키·휠과 마우스로 선택한 편집 메뉴의 실행 취소가 해당 작업만 pause하고 revision을 올리며 다른 작업을 유지하는 것을 확인했습니다. 모든 편집 메뉴 항목과 IME 조합 입력을 검증한 것은 아닙니다.

마지막 CEF 탭이 닫히면 앱도 종료되며 빈 창이나 작업 상태를 복원하지 않습니다. 작업은 최대 128개 기록, 작업당 소유 탭은 생성 예약을 포함해 4개로 제한합니다. 기록이 가득 차면 완료된 오래된 작업부터 제거합니다.

## 동작과 제한

- macOS AppKit 브라우저 창 하나에 CEF Alloy 페이지 뷰를 탭으로 넣습니다. Chromium 엔진은 유지하며 탭, 주소창, 이동 버튼, 작업 상태는 네이티브 UI입니다. CLI `open`, 페이지 팝업과 `target="_blank"` 링크를 같은 창의 새 탭으로 연결합니다. 백그라운드 새 탭 요청은 현재 선택을 유지합니다. Chrome 기본 omnibox, 확장 기능 관리 등의 UI는 제공하지 않습니다.
- 앱이 탭을 유지합니다. CLI는 요청 한 번을 보내고 종료합니다. 소켓 연결은 최대 16개까지 독립적으로 처리하며, 페이지 준비를 기다리는 동안에도 다른 CLI 요청을 받을 수 있습니다. CEF 조작은 UI 스레드에서 실행합니다. 16개를 넘는 동시 연결은 종료합니다.
- 모든 `read`는 새로운 요소 참조를 발급합니다. 이전 `read`의 참조와 페이지 이동 전 참조는 만료됩니다. 참조는 네이티브 탐색 세대에도 연결되므로 방문 기록에서 이전 문서를 복원해도 새 `read`가 필요합니다. 참조 문자열은 분해하거나 직접 만들지 않고 반환된 값 그대로 사용합니다. 페이지 이동 도중 준비 중이던 읽기·조작은 `PAGE_CHANGED`로 중단합니다. 실패한 클릭이나 입력은 자동 재시도하지 않습니다.
- 페이지별 격리된 JavaScript 실행 영역에서 메인 문서와 열린 Shadow DOM을 읽습니다. 중첩된 열린 root와 슬롯을 따라가며 비밀번호 입력값, 숨김·inert 영역은 반환하지 않습니다. 본문은 12,000자, 조작 요소는 120개입니다. 방문 노드 10,000개, 열린 shadow root 64개, 순회 깊이 64에서 탐색을 제한합니다. `page.truncated`의 `text`, `interactive`, `nodes`, `shadowRoots`, `depth`로 잘린 범위를 확인합니다. 이는 관찰 범위 제한이며 Chromium이 보유한 원본 DOM이나 메모리의 상한은 아닙니다.
- 후속 소스는 [접근 가능한 iframe 문서](docs/iframe-control.md)의 본문과 조작 요소를 함께 읽습니다. 문서 연결이 바뀌면 참조를 거절하고 전체 읽기 예산을 공유합니다. `page.frames`와 요소의 `frame`은 read 한 번의 문맥을 구분하며, 최대 16개와 중첩 8단계에서 제한합니다. 기존 앱 번들 반영과 실제 CEF의 다중 문서 입력 검증은 남아 있습니다.
- 첫 버전의 클릭과 입력은 DOM 이벤트입니다. 실제 사용자 입력과 같은 trusted 이벤트가 아닙니다. 열린 Shadow DOM에서도 반환된 참조로 클릭·입력·Enter를 요청할 수 있습니다. Enter는 입력 요소의 폼 제출을 요청하며, 결과는 다시 `read`해서 확인해야 합니다. 다른 키, 접근 불가능한 iframe 내부, 닫힌 Shadow DOM, 캔버스 내부는 미지원입니다.
- 로컬 소켓은 소유 사용자와 권한을 확인합니다. 소켓은 0600, 부모 폴더는 0700이어야 합니다. Chromium sandbox를 켜고 외부 디버깅 포트는 열지 않습니다.
- API는 로컬 앱 사용자를 위한 것입니다. 작업 소유권과 revision 검사는 같은 사용자 아래에서 명령 충돌을 줄이는 기능이며, 서로 다른 에이전트를 인증하거나 브라우저 프로필을 격리하는 보안 경계는 아닙니다. 본문 입력 감지는 OS 자동화 입력도 포함하므로 물리적인 사람의 입력만을 인증하는 기능이 아닙니다. 로그인한 페이지를 외부 모델에 전달하는 정책은 AI 연결 단계에서 추가해야 합니다.
- 웹페이지 본문은 비신뢰 데이터입니다. 페이지에 적힌 지시를 CLI 실행 권한으로 취급하면 안 됩니다.

## 메모리와 자원 관리

자동화의 새 탭 생성은 기존 CEF 탭과 생성 예약의 합이 8개에 도달하면 거부합니다. 작업별로는 4개까지 허용합니다. active 작업의 새 탭 링크에도 같은 예산을 적용합니다. 작업 소속이 없는 탭이나 paused 작업의 탭에서 여는 사용자 링크에는 자동화 생성 예산을 적용하지 않습니다. 기존 사용자 탭은 강제로 닫지 않으며, 이 숫자는 Chromium 전체 RAM의 상한이 아닙니다. CLI 스크린샷은 한 번에 하나만 캡처하고 디코딩 결과를 파일로 직접 저장합니다. 실행을 수락한 캡처의 자리는 시간 초과, handoff, 렌더러 종료 알림 뒤에도 실제 결과, DevTools 연결 종료 또는 탭 닫힘까지 유지합니다.

탭과 탐색 막대, 작업 상태 표시는 AppKit 컨트롤을 사용하므로 UI용 웹 renderer를 추가하지 않습니다. 상주 타이머 없이 변경 알림을 100ms 동안 모아 갱신하고, 창이 숨겨져 있거나 최소화돼 있으면 상태 스냅샷 생성을 건너뜁니다. 네이티브 UI에도 메모리가 필요하며, 숨은 탭의 Chromium 페이지를 자동 폐기하거나 RAM 절감률을 보장하지 않습니다.

공간 미리보기는 목록을 열었을 때 현재 카드 최대 8개의 대표 탭을 차례로 캡처합니다. CLI 스크린샷과 캡처 자리 하나를 공유하며 반복 타이머나 디스크 캐시는 없습니다. 수신 PNG는 이미지당 1MiB, 640×480 픽셀 이하만 디코딩합니다. 목록을 닫으면 카드와 UI 이미지 참조를 해제합니다. CEF 캡처는 최대 240×150 포인트를 목표로 축소 요청하지만 실제 Retina PNG의 픽셀 크기는 직접 측정하지 않았습니다. 앱 메모리의 측정 조건과 한계는 [메모리 관리 문서](docs/memory.md)를 참고합니다.

필요할 때 `python3 scripts/measure-memory.py --pid <앱_PID> --samples 3 --footprint`로 앱과 보조 프로세스를 함께 측정할 수 있습니다. RSS 합계는 공유 메모리를 중복 계산하므로 고유 RAM 사용량과 구분해야 합니다. 설계와 측정 조건은 [메모리 관리 문서](docs/memory.md)에 정리했습니다.

## 검증

```bash
export PATH="$PWD/.cache/bin:$PATH"
export CEF_PATH="$PWD/.cache/cef"
cargo test --locked --offline --lib
node --test tests/dom_bridge.test.mjs
python3 -m unittest discover -s tests -p '*_test.py'
python3 tests/smoke.py --cli target/release/lightbrowser --data-dir "$PWD/.local-data"
python3 tests/navigation_smoke.py --cli target/release/lightbrowser --data-dir "$PWD/.local-data"
python3 tests/controls_smoke.py --cli target/release/lightbrowser --data-dir "$PWD/.local-data"
python3 tests/tasks_smoke.py --cli target/release/lightbrowser --data-dir "$PWD/.local-data"
python3 tests/resources_smoke.py --cli target/release/lightbrowser --data-dir "$PWD/.local-data"
python3 tests/shadow_smoke.py --cli target/release/lightbrowser --data-dir "$PWD/.local-data"
python3 tests/single_window_smoke.py --cli target/release/lightbrowser --data-dir "$PWD/.local-data"
python3 tests/spaces_smoke.py --cli target/release/lightbrowser --data-dir "$PWD/.local-data"
```

Node는 DOM 단위 테스트에만 사용합니다. 앱과 CLI 실행에는 필요하지 않습니다.

통합 테스트는 실행 중인 앱에 접속합니다. 자체 로컬 HTTP 페이지를 띄워 페이지 읽기, 클릭, 입력, 실제 폼 탐색, 참조 만료, 비밀번호 제외, 격리 실행, 스크린샷을 확인합니다. 테스트가 만든 탭만 닫고 기존 탭은 남깁니다. 외부 사이트의 전체 호환성을 검증하는 테스트는 아닙니다.

`tests/shadow_smoke.py`는 작업 하나와 탭 하나에서 열린·중첩 Shadow DOM, 슬롯, root 내부 라벨, 입력·클릭·Enter, 숨김·제거 후 참조 거절, 제외할 내용과 순회 상한을 검사하도록 구성했습니다. 후속 소스에는 접근 가능한 iframe 입력과 opaque sandbox 제외 검사를 추가했으며 이 추가 검사는 아직 실행하지 않았습니다. 같은 탭으로 fixture를 이동하며 기존 사용자 탭과 프로필을 변경하지 않습니다.

`tests/single_window_smoke.py`는 CLI로 선택 메타데이터, 배경 탭 읽기·캡처, 작업 종료 후 다른 탭 보존을 검사합니다. 합성 DOM 클릭은 실제 사용자 입력과 같지 않으므로 `target="_blank"` 새 탭 생성과 opener 소유권은 네이티브 UI 검사로 분리합니다.

`tests/spaces_smoke.py`는 작업 생성과 배경 탭 생성의 선택 보존, 공간 전환, 빈 공간, adopt와 release의 원래 공간 복귀, 완료 결과 보존을 검사합니다. 기존 탭은 1~4개여야 하며 테스트끼리 동시에 실행하지 않습니다. 공간 카드, 실제 포커스, 접는 사이드바와 미리보기 이미지는 별도 네이티브 UI 검사로 확인합니다.

### 탐색 실패 안내와 복구 — v2 로컬 CEF·네이티브 확인

`src/page_recovery.rs`, `src/browser.rs`, `src/control_panel.rs`에 현재 페이지의 탐색 실패와 렌더러 종료를 알리는 네이티브 막대, **주소 수정**과 **새로 고침**을 구현했습니다. 주소 수정은 입력에 포커스를 옮기며 이 동작만으로 AI 작업을 인계하지 않습니다. 새로 고침은 사용자가 누를 때 기존 reload 경로를 실행하며 해당 탭의 AI 제어권을 확인합니다. 자동 재시도는 추가하지 않았습니다. 버튼 실행 전에 실제 선택 탭·표시 공간·탐색 세대·실패 종류가 그대로인지 다시 검사하고 로딩 중 재시도는 거절합니다.

v2는 같은 주소의 비사용자 후속 탐색에 한해 이전 오류 후보 하나를 내부에 보관하고, 진행 중 공개 오류는 비우며 같은 주소의 ERR_ABORTED가 오면 후보를 복원합니다. 명시적 탐색·사용자 제스처·redirect/history·다른 주소·성공 commit·중지·렌더러 종료에서는 후보를 지웁니다. 후보가 없는 ERR_ABORTED는 다른 오류로 추정하지 않습니다. 이 조건은 Chromium 내부 오류 페이지나 자동 재시도 콜백을 정확하게 식별한다는 뜻은 아닙니다. 진단 로그에는 주소 원문 대신 코드와 주소 일치 여부만 기록합니다.

v2의 Rust 189개·전체 타깃 Cargo 검사·format과 release 빌드가 통과했고 빌드 중간 파일 2.6GiB를 정리했습니다. 같은 앱의 첫 로컬 HTTP가 추가 승인 질문 없이 완료됐습니다. 사용자·배경 AI 오류를 각각 12초 이상 관찰한 복구 6개, 기존 선택·작업·공간·단일 창 회귀 32개, 추가 탐색 6개와 컨트롤 7개로 총 51개 로컬 CEF 검사를 통과했습니다. 자체 검사 탭을 정리한 뒤 원래 사용자 탭 1과 다운로드·대기·캡처·생성 예약 0을 확인했습니다. 근거는 `artifacts/navigation-recovery-v2-smoke.json`, `artifacts/navigation-recovery-v2-suite.json`, `artifacts/navigation-recovery-v2-suite-state.json`, `artifacts/navigation-recovery-v2-navigation-controlled.json`, `artifacts/navigation-recovery-v2-controls-smoke.json`입니다.

실제 네이티브 화면에서 실패 막대 유지, 주소 수정의 선택·포커스, 검색 화면에서 막대 숨김과 Esc 복원, 같은 주소의 새로 고침 성공과 막대 제거를 확인했습니다. AI A의 주소 수정은 A/B 모두 active revision 1을 유지했고, 새로 고침은 A만 paused revision 2로 바꾸면서 B는 active revision 1을 유지했습니다. 근거는 `artifacts/navigation-recovery-v2-native.json`입니다.

v1은 짧은 복구 검사 6개를 통과했지만, 이후 같은 주소의 지연 탐색에서 -324 ERR_EMPTY_RESPONSE가 -3 ERR_ABORTED로 바뀌고 막대가 사라지는 결함이 있었습니다. 변경 전 관찰과 최초 통과·실패 기록은 `artifacts/navigation-recovery-v1-baseline.json`, `artifacts/navigation-recovery-v1-smoke.json`, `artifacts/navigation-recovery-v1-native.json`에 보존합니다.

추가 탐색 검사는 최초에 오류용 포트가 즉시 연결 거절될 것이라는 fixture 전제를 사용했고 시간 초과로 실패했습니다. 별도 TCP 대조에서도 즉시 거절되지 않아 이 전제를 제거했습니다. Chromium 내부의 시간 초과 원인까지 확정한 것은 아닙니다. 요청을 받은 자체 서버가 응답 없이 연결을 닫는 방식으로 fixture를 수정한 뒤 같은 앱에서 탐색·컨트롤 검사를 통과했고 실제 실패 요청 접수와 원래 탭 보존을 확인했습니다. 최초 실패와 OS 대조 기록도 보존합니다. 실제 렌더러 종료와 복구, 좁은 창·다크 모드·IME·외부 사이트·메모리 절감·Preseller 내장은 미검증입니다. 전체 브라우저 완성이나 모든 재빌드의 키체인 승인 문제 해결을 뜻하지 않습니다.

### 최근 닫은 탭 다시 열기

내 공간에서 `⌘⇧T` 또는 `파일 → 최근 닫은 탭 다시 열기`로 최근 닫힌 웹 주소를 새 탭에 엽니다. 현재 실행 동안 사용자 공간의 HTTP(S) 주소 최대 10개만 기억하며 AI 소유 탭은 포함하지 않습니다. AI 공간·공간 목록·통합 검색·북마크 화면에서는 실행되지 않습니다. 폼 입력·스크롤·뒤로가기 목록은 복원하지 않고 앱 종료 시 기억한 주소도 사라집니다.

별도 파일이나 renderer를 보관하지 않습니다. [동작·검증 설계](docs/reopen-closed-tabs.md)에 범위와 실패 시 동작을 기록했습니다. v1의 AI 본문 단축키가 작업을 중지시키는 문제는 v2에서 수정했습니다. 같은 v2의 네이티브 검사에서 사용자 탭의 역순 복원, AI 작업의 active·revision 보존, 본문 키 전달 차단과 재시작 후 기록 초기화를 확인했습니다. Rust 144개·전체 타깃 검사와 로컬 CEF 회귀 22개도 통과했습니다. 검증한 SHA-256은 `84bfaf36d80d6b6048461b07fcbef5c5bca2862c3408aa7d2e772e1fa5204100`이며 원본·정리·한계는 `artifacts/reopen-v2-validation.json`에 있습니다. 실제 IME 조합·외부 사이트·장시간 사용은 후속 검증입니다.

### 페이지 내 찾기

`⌘F`로 주소줄 아래의 찾기 막대를 엽니다. 입력하면 CEF가 현재 위치를 기준으로 일치 항목을 선택하고 결과 수를 표시합니다. Enter·⌘G는 다음, ⇧Enter·⇧⌘G는 이전, Esc는 닫기입니다. 같은 문서에서는 검색어 하나를 기억하지만 탭·문서·공간이나 다른 목록 화면으로 전환하면 초기화합니다. AI가 제어 중인 페이지에서 찾기를 열면 해당 작업을 중지합니다.

검색어는 최대 200자이고 후속 요청은 최신 한 건만 대기시킵니다. 별도 renderer, 저장 파일, 상주 worker를 추가하지 않습니다. [페이지 찾기 설계](docs/page-find-design.md)에 구현과 검증 범위를 기록합니다.

v1의 실제 로컬 화면에서 영문 8개·한글 5개, iframe 강조, 다음·이전·순환, 입력 포커스와 탭·탐색 전환을 확인했습니다. 로딩 대기 중 네이티브 찾기는 해당 AI 대기를 취소했습니다. AI 재개에서 이전 찾기가 남는 결함을 발견해 수정했으며 Rust 137개와 v2 release 빌드가 통과했습니다.

페이지 찾기 v2 검증에 사용한 SHA-256은 `458ee5ad7a075d2a0b3c35b1d75f037847976e7711720b7319529eea1e0ea427`입니다. 사용자 키체인 승인 후 재빌드 없이 같은 앱의 HTTP 로딩과 수정 동작을 확인했습니다. A의 잘못된 revision 재개와 B의 재개는 A의 검색·포커스를 보존했습니다. A를 정상 재개하면 막대와 화면 강조가 사라지고 `⌘G`가 이전 검색어를 되살리지 않았습니다. A/B의 active·revision 3도 유지했습니다. 다시 `⌘F`를 열면 빈 입력과 새 handoff로 시작했습니다. 근거는 `artifacts/find-page-v2-native.json`입니다.

같은 v2의 로컬 CEF 작업·공간·단일 창 22개가 모두 exit 0·빈 stderr로 통과했습니다. 생성한 작업 7개를 종료하고 기존 빈 탭 1과 대기·캡처·생성 예약 0을 확인했습니다. 근거는 `artifacts/find-page-v2-suite.json`과 `artifacts/find-page-v2-suite-state.json`입니다. 최초 샌드박스 읽기의 소켓 권한 오류는 별도 `*-sandbox.json`에 보존했으며 권한을 받은 재시도에서 통과했습니다.

네이티브 메뉴로 정상 종료한 뒤 exit 0을 확인하고, 같은 프로필·바이너리로 재실행해 추가 승인 없이 로컬 HTTP 로딩과 빈 북마크 목록을 확인했습니다. 이전 앱과 검증 서버를 정리하고 원래 빈 탭 하나를 남겼습니다. 검사 후와 재시작 후 RSS 표본은 [메모리 관리 문서](docs/memory.md)에 있으며, 공유 매핑과 조건 차이 때문에 전후 절감량으로 해석하지 않습니다. IME 조합·좁은 창·외부 사이트·장시간 사용과 Preseller 내장은 미검증입니다. 이번 키체인 대기는 해소됐지만 전체 브라우저 완성을 뜻하지는 않습니다.

### 다운로드 구현과 검증 상태

완료 콜백의 경로가 사용자가 승인한 저장 경로와 일치한 파일에 **Finder에서 보기**를 소스로 추가했습니다. 파일을 실행하지 않고 위치 표시만 요청하며 기존 접수 탭 보기와 별개입니다. 파일 이동과 삭제를 추적하지 않으며 기존 앱 바이너리에는 아직 반영하지 않았습니다. [다운로드 설계](docs/downloads-design.md)에서 검증 범위를 구분합니다.

파일 카드의 **접수 탭 보기**, 권한 카드의 **요청 탭 보기**를 소스로 추가했습니다. 기존 공간과 탭만 선택하며 AI 작업 중지, 권한 승인이나 파일 저장을 실행하지 않습니다. 파일 접수 이후 페이지나 제어권이 바뀌었으면 이동을 거절합니다. 중지된 AI 공간도 실제 작업 이름과 탭 번호로 표시합니다. 숨긴 Chromium 권한의 자동 감지는 아니며 추가 파일이 나타나지 않을 때 사용자가 직접 접수 탭을 확인하는 흐름입니다. 기존 앱에는 아직 반영하지 않았고 [다운로드 설계](docs/downloads-design.md)에 계약과 실제 앱에서 남은 검증을 기록했습니다.

현재 사이트의 저장된 자동 다운로드 허용을 확인하고 기본값으로 초기화하는 기능을 소스로 추가했습니다. 다운로드 목록의 고정 카드에서 허용 상태일 때 실행할 수 있으며 모든 공간의 같은 사이트에 적용됩니다. 대기 요청은 취소될 수 있고 진행 중인 파일은 유지합니다. 기존 문서의 내부 허용 상태까지 즉시 철회하는 기능은 아니므로 열린 페이지는 다시 열어 확인하도록 안내합니다. 새 저장소나 승인 목록 캐시는 없으며 [동작 계약](docs/download-site-settings.md)에 검증 범위를 기록했습니다. 기존 앱 바이너리에는 아직 반영하지 않았습니다.

사이드바나 보기 메뉴에서 다운로드 목록을 엽니다. 사용자와 AI 페이지의 요청은 먼저 대기하며, 목록에서 저장 위치 선택을 누르면 네이티브 저장창이 열립니다. 진행 중인 파일은 취소할 수 있고 완료 기록 지우기는 파일을 삭제하지 않습니다. 대기 개수는 사이드바에도 표시합니다.

한 사이트가 여러 파일을 요청하면 다운로드 목록에 사이트 주소와 허용 버튼을 표시합니다. 허용은 해당 사이트 설정으로 저장되며 다른 공간에도 적용됩니다. 이번 요청 취소는 영구 차단을 남기지 않습니다. 권한 대기는 최대 4개이고 각 파일의 저장 위치 선택은 별도로 유지합니다. 요청이 생겨도 현재 공간이나 포커스를 바꾸지 않습니다. `status`의 `downloads.active`, `downloads.awaitingSave`, `downloads.awaitingPermission`으로 진행, 저장 위치 대기, 권한 대기를 구분할 수 있습니다. 현재 사이트 허용 초기화 소스의 실제 앱 검증은 아직 남아 있습니다.

Chromium은 숨겨진 탭의 권한 표시를 보류하므로 해당 공간을 직접 열어야 카드가 나타날 수 있습니다. `awaitingPermission`에는 Naru가 실제로 전달받은 카드만 포함됩니다. 다운로드를 요청한 문서에서 제어권이 바뀌면 늦게 도착한 요청을 새 사용자 요청으로 승인하지 않습니다. 이 경우 같은 문서의 새 여러 파일 권한 요청도 취소될 수 있으며, 필요한 작업은 직접 새로 고친 뒤 다시 시작해야 합니다. 자동 새로고침은 하지 않습니다.

대기와 진행, 취소 요청 중인 항목을 합쳐 4개로 제한합니다. 목록은 최대 24개이며 재시작하면 비웁니다. 활성 AI 작업에서 시작한 다운로드는 제어권 인계와 작업 종료에 따라 취소합니다. 별도 UI renderer나 영구 이력 저장소를 추가하지 않았습니다. 이전 다운로드 v3에서 실제 파일 저장과 해시 일치, 저장창과 전송 중 취소, 파일 선택, AI 작업 중지 뒤 늦은 저장 거절, 탭 닫기 취소 시 다운로드 보존을 확인했습니다. 재시도 결함을 수정한 다운로드 v4는 Rust 164개, 전체 타깃 검사와 기존 CEF 회귀 32개를 통과했습니다. v4에서 중단 상태 표시, 정상 저장, 전송 중 취소, 기록 삭제와 파일 보존, 재시작을 확인했습니다.

후속 권한 v2는 Rust 174개, 전체 타깃 검사, format과 release 빌드를 통과했습니다. 첫 HTTP의 키체인 대기가 해소된 뒤 같은 바이너리에서 로컬 HTTP 완료와 권한 수명 회귀 4개를 확인했습니다. 자체 작업 공간을 명시적으로 열어 전달받은 요청의 인계와 탐색 취소, 숨긴 요청의 adopt와 handoff 이후 늦은 표시 거절을 검사했습니다. 실제 네이티브 화면에서 카드 순서와 이번 요청 취소, 닫기 버튼으로의 포커스 이동도 확인했습니다. 같은 v2의 기존 CEF 회귀 32개도 통과했고 검사 작업을 정리해 원래 사용자 탭 1과 다운로드 0개를 보존했습니다. 동일 v2의 후속 네이티브 검사에서 허용 후 두 파일의 저장과 원본 바이트 일치를 확인했습니다. paused 탭의 사용자 요청 뒤 AI를 재개하면 권한 대기는 취소되고 먼저 접수한 사용자 파일의 저장 대기는 유지됐습니다. 외부 사이트는 미검증입니다. 배경 자동 권한 알림과 사이트 권한 해제 UI도 남아 있습니다. [설계와 검증 기록](docs/downloads-design.md)을 참고하세요.

### 검색·미리보기 v3·v2와 북마크 v1의 이전 검증 범위

이전 검색·미리보기 v3(`93bbf74352f3565713a24b4c9b20729e2b7bb627f8b771c7f91f35ebb9b9a9bd`)입니다. Rust 128개와 전체 타깃 Cargo 검사, release 빌드가 통과했습니다. `artifacts/search-preview-v3-build.log`에 빌드와 중간 파일 4,332개·2.4GiB 정리를 기록했습니다. 이전 v2(`f9168ced388db274d3d18538a792081c7b5302bffc6b7817b4a59616c9227f74`)는 아래 검증을 마친 뒤 exit 0으로 종료했습니다.

`artifacts/search-preview-v2-native.json`에는 승인 전 첫 로컬 탐색의 5초 `WAIT_TIMEOUT`과 사용자 승인 후 같은 바이너리의 결과를 함께 기록했습니다. 승인 후 통합 검색에서 같은 숫자 ID인 탭 3과 북마크 3을 독립된 결과로 표시했고, 방향키·Enter·마우스 선택, 같은 사용자 탭 재사용과 새 사용자 탭 생성에서도 AI 작업의 active·revision 1·선택 탭 3을 유지했습니다. 일치 결과가 없을 때 Enter와 Tab·ShiftTab 이동도 확인했습니다. 한국어는 붙여넣기로 입력했습니다.

v2에서는 검색 입력 중 기억된 현재 탭 2를 닫을 때 결과와 검색어는 유지하지만 포커스가 창으로 이동하는 결함이 재현됐습니다. v3는 제거하는 CEF 뷰 안에 실제 포커스가 있을 때만 해제하도록 바꿨습니다. 빈 탭의 통합 검색과 빈 북마크 목록에서 탭을 닫아도 입력 포커스와 추가 붙여넣기를 유지했습니다. 첫 HTTP 탐색의 5초 `WAIT_TIMEOUT` 후 사용자가 키체인을 항상 허용했고, 같은 v3 바이너리에서 HTTP 탭 3의 로딩 완료를 확인했습니다. 이번 승인 대기는 해소됐으며 재서명 후 승인 요청 자체를 없앤 결과는 아닙니다.

`artifacts/search-preview-v3-native.json`에서 사용자 탭 3·AI 탭 5·북마크 4의 혼합 검색을 확인했습니다. 사용자 탭 3을 닫아도 검색 필드 포커스와 추가 입력을 유지했습니다. “저장”으로 찾은 북마크를 Enter로 열면 사용자 탭 6을 만들고 AI 작업의 active·revision 1·선택 탭 5를 보존했습니다. 이어 `⌘D` 편집 중 탭 6을 닫아도 이름 입력의 포커스·선택과 고정 URL을 유지했고, “초안 유지”로 저장한 파일에서 북마크 ID 4와 원래 URL을 확인했습니다. 검증 북마크는 UI로 삭제해 `items: []`, `nextId: 5`로 정리했고 AI 작업도 종료했습니다. 한국어는 붙여넣기 검증이고 IME 조합 검증은 아닙니다.

같은 v3의 로컬 CEF 작업 6개·공간 10개·단일 창 6개, 총 22개가 통과했습니다. 세 묶음 모두 exit 0이며 stderr는 비어 있습니다. `artifacts/search-preview-v3-suite.json`과 `artifacts/search-preview-v3-suite-state.json`에 결과를 기록했습니다. 최종 상태는 기존 사용자 빈 탭 1 하나와 `overview:false`였고, 대기 요청·캡처·탭 생성 예약은 모두 0이었습니다. 회귀 검사가 만든 작업 7개는 모두 `finished`이며 `tabs`와 `closingTabs`가 비어 있습니다. 이후 v3를 ⌘Q로 정상 종료해 exit 0을 확인하고 같은 바이너리로 재실행했습니다. 빈 사용자 탭 하나와 빈 북마크 목록을 확인했으며 백업의 삭제 항목이 되살아나지 않았습니다. 종료·재실행 근거는 `artifacts/search-preview-v3-validation.json`과 네이티브 기록에 있습니다. 재실행 후 HTTP 탐색과 새 RSS 측정은 하지 않았습니다.

v3의 최초 샌드박스 단위 검사에서는 소켓 bind 권한 오류 3건이 발생했습니다. 재실행은 Rust 128개를 통과했습니다. 각각 `artifacts/search-preview-v3-rust-tests-sandbox.log`, `artifacts/search-preview-v3-rust-tests.log`에 보존했고 전체 타깃 검사 기록은 `artifacts/search-preview-v3-check.log`입니다.

`artifacts/search-preview-v2-delays.json`에서는 빠른 문서의 파란 미리보기와 응답 헤더 대기·이미지 로딩 중인 문서의 안내 화면을 확인했습니다. 각 대기를 POST로 해제한 뒤 공간 목록을 다시 열지 않아도 주황 문서, 초록 문서와 SVG가 차례로 표시됐습니다. 로컬 CEF 작업·공간·단일 창 22개는 `artifacts/search-preview-v2-suite.json`에서 통과했고, 정리 후 캡처·생성 예약 0과 기존 사용자 탭 보존은 `artifacts/search-preview-v2-suite-state.json`에 있습니다. 검증용 작업을 종료하고 북마크 3을 삭제해 본문은 `items: []`, `nextId: 4`가 됐습니다. 새 RSS 측정, IME 조합, 외부 서비스와 장시간 사용 결과는 없으며 과거 승인 대기나 캡처 정체의 원인을 확정한 검증도 아닙니다.

아래는 이전 북마크 v1 바이너리(`bd7baf100fd434e0898fa98c9453988fb5b531ed205d5eea86f4e27f6c828dbd`)의 실행 증거입니다. `artifacts/bookmarks-v1-rust-tests.log`의 Rust 검사 124개, release 빌드와 `artifacts/bookmarks-v1-suite.json`의 로컬 CEF 작업·공간·단일 창 검사 22개가 통과했습니다. v2 결과로 합산하지 않습니다.

`artifacts/bookmarks-v1-native.json`에서 사용자 승인 후 같은 앱의 `⌘D` 추가, 빈 이름 실패 후 초안 유지, 이름 변경과 중복 ID 유지, 한국어·URL 검색, 방향키·Enter를 확인했습니다. 기존 사용자 탭 재사용과 새 사용자 탭 생성 모두 AI 작업의 active·revision 1·선택 탭 3을 유지했습니다. A의 편집 화면을 연 뒤 원래 탭을 B로 탐색해도 고정한 URL A의 이름만 바뀌었습니다. 정상 `⌘Q` 종료는 exit 0이었고, 재실행에서 두 항목의 이름·ID·순서를 보존했습니다. 마지막 항목 삭제 후 다시 실행해도 정상 빈 본문을 사용하며 백업의 옛 항목을 되살리지 않았습니다.

`artifacts/bookmarks-v1-recovery.json`은 검증용으로 만든 손상 프로필의 읽기 전용 안내, 추가·이름 수정·삭제 비활성화, 백업 항목 열기를 기록합니다. 본문과 백업의 해시는 검사 전·후와 정상 종료 후에 같았습니다. exit 0을 확인한 뒤 해당 임시 프로필만 삭제했습니다. 복구 파일을 다시 쓰는 UI나 외부 서비스, IME 조합, 장시간 부하를 검증한 결과는 아닙니다.

### 일상 탐색 v6 검증

2026-10-11 v6 바이너리(`7f855dcd1cfaf91523c631c5d6b083e87663d43d06608d07edc2a3e41710f546`)의 Rust 단위 검사 117개와 로컬 CEF 작업·공간·단일 창 검사 22개가 통과했습니다. 근거는 `artifacts/daily-browser-v6-rust-tests.log`와 `artifacts/daily-browser-v6-suite.json`입니다. 저장 모듈 검사는 포함하지만 새 북마크 UI와 비동기 런타임 연결은 이 바이너리에 포함되지 않습니다.

사용자 키체인 승인 후 같은 v6 바이너리에서 로컬 페이지 주소 입력과 탐색, 잘못된 주소 보존·오류 표시, 이후 본문 링크로 이동했을 때 주소와 오류 갱신, 검색 열기·Esc 다섯 번 반복을 확인했습니다. 한국어 제목 검색으로 기존 탭을 선택할 때 AI 작업을 유지했고, 본문 클릭·키·휠·편집 메뉴 실행 취소에서 해당 작업만 인계되는 것도 확인했습니다. `artifacts/daily-browser-v6-native.json`에 입력 결과와 revision, 다른 작업 유지 및 검증 탭 정리 결과를 기록했습니다. 한국어는 붙여넣기로 입력했으며 IME 조합, 나머지 편집 메뉴, 실제 입력 중 대기 요청 취소, 외부 사이트와 장시간 사용은 검증하지 않았습니다.

v6는 북마크 UI 이전 기록입니다. 위 북마크 v1의 실행 결과 및 검색·미리보기 v2 검사와 합산하지 않습니다.

### 공간 UI v3 검증

2026-10-10 최종 공간 UI 바이너리의 SHA-256은 `710ec71ecfcb0fa374d875eba0ad24492b3a6d63d603e8a0da455bac0d7a8cca`입니다. Rust 단위 테스트 88개, 모의 DOM 테스트 20개, Python 격리 테스트 22개가 통과했습니다. 로그는 `artifacts/spaces-v3-rust-tests.log`, `artifacts/spaces-v1-dom-tests.log`, `artifacts/spaces-v1-python-tests.log`에 있습니다.

실제 CEF와 로컬 HTTP를 사용한 자동 회귀는 기존 7개 묶음 44항목과 공간 테스트 10항목을 합해 8개 묶음, 54항목을 통과했습니다. 근거는 `artifacts/spaces-v3-suite.json`과 `artifacts/spaces-v3-spaces-smoke-after-allow.json`입니다. 사용자가 키체인에서 항상 허용한 뒤 같은 최종 바이너리로 로컬 페이지 로딩과 테스트를 완료했습니다.

별도 네이티브 검사는 정리 확인을 포함해 16개 수기 항목을 기록했습니다. 사용자 페이지를 유지한 배경 작업 읽기, 서로 다른 세 페이지의 실제 카드 미리보기, 배경 탐색 뒤 미리보기 갱신, 공간별 선택 탭 복원, 실제 페이지 버튼의 `window.open`, 빈 작업과 페이지가 있는 작업의 중지, 사이드바 접기와 단축키를 확인했습니다. 팝업은 원래 URL과 선택을 유지하며 같은 공간의 task 소유 탭으로 생겼습니다. 중지 뒤 revision 증가와 이전 revision 거절도 확인했습니다. 근거는 `artifacts/spaces-v3-native.json`입니다.

이 v3 기록에는 미리보기 PNG의 실제 픽셀 크기와 디코딩 할당량, 8개 카드와 여러 목록 페이지의 부하, 해당 버전의 마지막 탭 종료 검사가 없습니다. ⌘L은 주소창 선택과 문자열 변경까지만 확인했고 주소 탐색은 제출하지 않았습니다. 페이지 본문 입력 자동 handoff와 네이티브 주소 탐색의 후속 확인은 위 v6 기록에 있으며, 외부 사이트 전체 호환성과 배포 검증은 여전히 포함하지 않습니다.

### 공간 분리 이전의 단일 창 v2 기록

공간 분리 이전인 2026-10-10 단일 창 v2 검증에서 Rust 단위 테스트 75개, Python 격리 테스트 22개, 모의 DOM 테스트 20개가 통과했습니다. 실제 CEF 회귀는 로컬 HTTP를 대상으로 7개 테스트 묶음, 총 44개 항목이 통과했습니다. 결과와 바이너리 SHA-256은 `artifacts/single-window-v2-validation.json`에 기록했습니다. 이 기록은 새 공간 UI의 실행 결과가 아닙니다.

이 중 단일 창 테스트 6항목은 선택 메타데이터, 배경 읽기·캡처, 작업 종료 시 다른 탭 보존과 자원 반환을 확인했습니다. 같은 바이너리의 별도 네이티브 검사에서는 실제 `target="_blank"` 클릭으로 원래 탭 URL과 선택을 유지하면서 새 탭을 만들고 opener의 작업 소유권을 보존했습니다. 작업 중지 뒤 paused 표시, revision 1→2와 이전 revision 거절도 확인했습니다.

⌘T로 새 탭 생성, ⌘L로 주소창 포커스·문자열 변경, 초기 탭만 닫아도 다른 탭 유지, ⌘W로 마지막 탭을 닫은 뒤 프로세스 exit 0을 확인했습니다(`artifacts/single-window-v2-native.json`). ⌘L 검사에서 주소 탐색은 실행하지 않았으며 이 바이너리의 beforeunload 취소 성공은 주장하지 않습니다. 검증 범위는 [설계 문서](docs/architecture.md)에 정리했습니다.

이전 단일 창 v2 통합 테스트 후 빈 탭 하나에서 6개 프로세스를 3회 측정한 RSS 합계는 714.453MiB였습니다. 공유 매핑을 중복 계산하므로 고유 RAM 사용량이 아니며, physical footprint는 경고 때문에 유효한 값을 얻지 못했습니다. 새 공간 UI의 측정값이 아니며 같은 조건의 변경 전 비교가 없어 절감량은 주장하지 않습니다. 측정 파일과 조건은 [메모리 관리 문서](docs/memory.md)를 참고합니다.

이전 빌드의 검증과 실패 기록은 [개발 검증 기록](artifacts/development-validation.json), [이전 단일 창 실행](artifacts/single-window-release-validation.json), [초기 실패 보고서](artifacts/single-window-result.json)에 보존하며 현재 결과에 합산하지 않습니다.

위 결과는 로컬 개발 환경의 기록이며 GitHub CI, 배포나 외부 사이트 전체 호환성 검증은 아닙니다. 실제 사이트 로그인 유지, 이전 프로필 이전, 결제, 다운로드, DRM 콘텐츠와 재서명 후 무승인 실행은 검증하지 않았습니다. 보고서·스크린샷은 개발 환경의 `artifacts/`에 보관하고, 프로필·쿠키·로그·다운로드 캐시·앱 번들과 함께 Git 업로드 대상에서 제외합니다.

재시작 검증은 존재하지 않는 전용 데이터 경로를 사용합니다. 두 번 모두 정상 종료하고 검증에 성공하면 테스트가 만든 `profile-v1`을 삭제하며, JSON 보고서·로그·스크린샷은 남깁니다. 실패한 프로필은 진단을 위해 보존합니다. 성공 후에도 프로필을 조사해야 할 때만 `--keep-profile`을 지정합니다.

```bash
python3 tests/restart_smoke.py --app 'dist/Light Browser.app' \
  --cli target/release/lightbrowser --data-dir .local-data/restart-new
```

## 소스 구성

- `src/browser.rs`: CEF 수명주기, 명령 처리, 내부 DevTools API 연결
- `src/protocol.rs`, `src/cli.rs`: 로컬 소켓과 CLI
- `src/tasks.rs`: 작업 상태, 탭 소유권, 제어권 revision, 종료 계획
- `src/spaces.rs`: 표시 공간, 공간별 선택, 빌린 탭의 복귀 위치
- `src/control_panel.rs`: 단일 AppKit 브라우저 창, 세로 탭, 공간 카드, 탐색 막대, 작업 상태와 중지
- `src/history.rs`, `src/history_runtime.rs`: 제한된 방문 기록 파일, 비동기 저장, 삭제 세대와 검색
- `src/dom_bridge.js`: DOM 스냅샷과 요소 조작
- `src/mac.rs`, `src/bin/helper.rs`: macOS 앱 이벤트와 CEF sandbox helper
- `scripts/`: 의존성 준비, 앱 번들 생성과 실행

런타임은 CEF `156.0.1+g8db2f36+chromium-156.0.8078.17` beta, API는 `15600`으로 고정했습니다. 공식 cef-rs 생성기로 macOS 바인딩을 다시 만든 로컬 파생본을 `vendor/cef-rs`에 포함했습니다. 배포된 cef-rs 156 정식 패키지가 아니며 [생성 근거와 재현 방법](vendor/cef-rs/README.local.md)을 기록했습니다. 앱 시작 시 실제 API 해시를 확인하고, 번들 생성 전에 헤더·메타데이터·프레임워크 버전을 교차 확인합니다.
