# Naru 아키텍처와 구현 단계

Naru는 사람이 사용하는 Chromium 브라우저를 로컬 CLI에서도 읽고 제어하는 macOS 앱이다. Rust 앱이 브라우저와 작업 상태를 보관한다. CLI는 요청 한 번을 전달하고 종료한다. 앱 실행에 Node 런타임이나 MCP 서버는 필요하지 않다.

프로젝트 이름은 Naru지만 현재 앱 번들, 실행 파일, 프로필 식별자는 `Light Browser.app`, `lightbrowser`, `local.lightbrowser.dev`를 사용한다. 이름 변경과 프로필 이전은 별도 작업이다.

## 완료 다운로드의 파일 위치 표시

`updated → Runtime::update → Model::update_with_approval`은 이번 Complete 경로가 승인된 Save As 경로와 일치할 때만 완료 기록의 확인 bit를 설정한다. 기존 terminal 불변 정책과 경로를 재사용하며 승인 경로의 활성 콜백 보관 상한은 유지한다. 현재 행, 창, 패널과 입력기 검증을 통과한 버튼만 `reveal_file → mac::reveal_file → NSWorkspace`로 요청한다. 파일 실행, 브라우저 선택과 AI 제어권 변경, 새로운 영속 저장은 없다. 반환값과 실제 Finder 선택의 증거 범위를 [다운로드 설계](downloads-design.md)에 구분한다.

## 제품 목표와 재사용 경계

Naru를 먼저 독립 브라우저로 완성한 뒤, 같은 Rust·CEF 브라우저 기능을 Preseller 내부에 내장한다. 독립 앱의 사용성과 CLI를 계속 개발하며 창·앱 수명주기와 공통 브라우저 구현의 의존성을 나눌 준비를 한다. 현재 외부 호스트를 받는 API나 공통 코어 패키지는 없으며, 아래 요청 흐름은 독립 앱의 실제 구현이다. 후속 구조와 완료 기준은 [독립 완성과 내장 계획](preseller-integration.md)을 따른다.

## 현재 구현과 후속 범위

| 영역 | 현재 구현 | 후속 범위 |
| --- | --- | --- |
| 브라우저 | Rust, CEF Alloy, Chromium, 단일 AppKit 창, 접는 세로 탭과 공간 카드 | 공간 전환과 창 수명주기의 실행 검증 확대 |
| 표시 공간 | 내 공간과 작업별 공간, 공간별 선택, 요청 시 대표 탭 미리보기 | 여러 공간의 사용성 검증 |
| 제어 | 로컬 CLI, 탐색, DOM 조작, 캡처, 작업 상태와 중지 버튼, 본문 입력 시 해당 AI 작업 인계 | 실제 IME와 네이티브 입력 중 대기 요청 취소 검증 |
| 관찰 | 메인 문서와 열린 Shadow DOM, 참조, viewport, 탐색·렌더러 상태. 접근 가능한 iframe 관찰·입력과 교차 출처 프레임의 읽기·7개 입력 API는 소스 연결, 최신 실제 앱 미검증 | 지원 밖 좌표 변환, 실제 CEF·E2E 검증 |
| 입력 | 위치 재검사 후 Chromium 클릭과 hover, 포커스 확인 후 42개 키 형식, 텍스트 fill의 Chromium 입력과 값 확인, 나머지 DOM 입력, 제어권 변경 시 요청과 참조 만료 | 최신 CEF 통합, 일반 조합키와 IME, 외부 편집기 호환성 |
| 제어권 | 작업별 소유권과 revision, CLI의 handoff와 resume, 네이티브 탐색과 본문 클릭, 키와 휠 입력 전 handoff | 외부 서비스와 복합 입력 경합 검증 |
| AI | 외부 프로그램이 CLI 사용 가능, 내장 모델 연결 없음 | 모델 연결, 페이지 정보 전송과 실행 승인 정책 |
| 배포 | 프로젝트 안에서 기본 빌드만 수행, 명시적 실행과 서명자 검사, 산출물 정리 | 지속 서명, 공증, 배포와 업데이트 |

작업 공간은 탭을 묶는 논리적 단위다. 별도 브라우저 프로필이나 별도 로그인 세션을 만들지 않는다. 모든 작업은 같은 앱의 프로필을 공유하므로 쿠키와 로그인 상태도 공유한다. 작업 소유권은 같은 macOS 사용자 안에서 명령 충돌을 줄이기 위한 규칙이며, 서로 신뢰하지 않는 사용자나 프로그램을 격리하는 인증 수단은 아니다.

## 요청과 실행 흐름

```mermaid
flowchart LR
    C[외부 도구 또는 사람] --> CLI[Rust CLI]
    CLI --> S[로컬 Unix 소켓]
    S --> V[요청 검증과 작업 revision 확인]
    P[단일 AppKit 브라우저 창의 작업 중지 버튼] --> V
    V --> UI[CEF UI 스레드]
    UI --> B[CEF Alloy의 Chromium 탭]
    UI --> D[내부 DevTools와 격리 DOM 영역]
    B --> R[콜백과 탐색 상태]
    D --> R
    R --> J[JSON 응답]
    J --> CLI
```

`src/bin/cli.rs`가 `src/cli.rs`의 인자 파서를 호출한다. `src/protocol.rs`는 요청과 응답 형식을 정의하고 소켓 연결, 사용자와 파일 권한 검사를 담당한다. 요청 하나마다 연결 하나를 사용하며, CLI 프로세스가 끝나도 앱의 탭과 작업은 유지된다.

`src/main.rs`는 프로필, 소켓, CEF 런타임을 준비한 뒤 `src/browser.rs`의 서버와 CEF 메시지 루프를 시작한다. 소켓 요청은 CEF UI 스레드에 전달된다. 브라우저 상태 변경과 작업 소유권 검사는 이 스레드에서 처리해야 한다. 대기 중인 요청이 다른 연결을 막지 않도록 소켓 연결은 독립적으로 처리한다.

연결 worker는 최대 16개다. 이와 별개로 UI에 게시한 요청 payload도 서버별 최대 16개로 제한한다. 연결이 45초 후 시간 초과돼도 미실행 요청은 자리를 유지하며, UI 실행 종료나 task의 마지막 참조 해제 때 반환한다. 상한에 도달한 새 요청은 게시 전에 `APP_BUSY`, `outcomeUnknown:false`로 거절한다. 게시한 요청의 시간 초과는 기존 `outcomeUnknown:true`를 유지한다. 새 worker나 반복 타이머는 추가하지 않는다.

페이지 관찰과 조작은 `src/browser.rs`의 내부 DevTools 요청에서 시작한다. 기존 `read`와 입력은 메인 프레임의 격리 실행 영역에서 `src/dom_bridge.js`를 실행한다. 별도 `read-frame`은 현재 탭에서 찾은 프레임의 정확한 DevTools session과 격리 context에 내용 조회와 문서에 묶인 참조를 연결한다. 그 참조의 입력 7개는 같은 원 context를 다시 확인한 뒤 처리한다. 포인터의 native 전송만 루트 session 좌표를 사용한다. 외부 디버깅 포트를 열지는 않는다. 페이지가 정의한 같은 이름의 JavaScript 객체를 신뢰하지 않는다.

`src/mac.rs`는 macOS 앱 이벤트와 브라우저 단축키를 연결하고, `src/control_panel.rs`는 단일 AppKit 브라우저 창을 구성한다. 창 안의 중지 버튼은 `src/browser.rs`의 기존 TaskHandoff 처리 경로를 호출한다. `src/bin/helper.rs`는 CEF 보조 프로세스와 sandbox 초기화를 담당한다. 런타임은 고정된 CEF 156.0.1 beta이며 로컬 파생 바인딩과 API 15600을 함께 사용한다. 버전 교차 검증과 앱 시작 시 API 해시 확인은 서로 다른 CEF 조합의 실행을 막는다.

`src/tasks.rs`는 CLI 작업 소유권과 revision을 관리한다. `src/spaces.rs`는 표시 공간, 공간별 탭 선택과 실행 중 표시 순서, 빌린 탭의 원래 공간을 관리한다. 표시 순서는 `SpaceView.tabs`와 shell snapshot에 공통으로 전달하며 같은 공간의 메뉴 이동과 드래그 정렬은 선택, 탐색, AI 인계 없이 메타데이터만 바꾼다. 드래그는 시작 당시 전체 순서를 재검사하고 지정한 탭 앞이나 목록 끝으로 이동한다. [탭 순서 계약](tab-order.md)에 메뉴 대상 수명과 자원 제한을 기록했다. `src/browser.rs`가 작업 생성과 소유권 변경 뒤 두 상태를 맞추고 `src/control_panel.rs`에 스냅샷을 전달한다. 표시 공간과 작업 소유권은 별개다. 완료 공간에 남긴 결과 탭은 작업 소유권 없이도 그 공간에 표시할 수 있다.

네이티브 화면의 `shell_snapshot`은 내부 상태를 빌리지 않은 구간에서 CEF getter를 호출한다. 각 조회와 최종 조립 시점의 탭, 문서와 로딩 세대를 검증하며 변경되면 기존 화면을 유지한 채 갱신을 보류한다. [상태 읽기 계약](shell-snapshot.md)에 중첩 조회와 CLI, AppKit의 남은 경계를 구분한다.

## 작업 공간과 제어권

작업은 `id`, `name`, `status`, `revision`, `tabs`, `closingTabs`, `releasedTabs`를 가진다. `tabs` 항목은 `{tab, origin}` 형식이며 `origin`은 `created` 또는 `borrowed`다. 나머지 두 배열에는 탭 ID가 들어간다.

| 명령 | 의미 |
| --- | --- |
| `task create NAME` | revision 1인 active 작업 생성. 같은 이름의 active/paused 작업이 있으면 새로 만들지 않고 그 작업을 `resumed:true`로 반환(paused는 상태·revision 변경 없이 반환하므로 제어권을 가져가지 않으며 `task resume`을 따로 호출). finishing/finished는 이어받지 않음 |
| `task wait ID [TIMEOUT_MS] [--since REV]` | 호출 시점의 작업 상태(상태, revision, 소유 탭, 닫는 중인 탭)가 바뀔 때까지 대기(기본 10000, 1..30000ms). `--since REV`는 현재 revision이 다르면 기다리지 않고 즉시 `changed:true`. 바뀌면 `{task,changed:true}`, 제한 시간이면 `WAIT_TIMEOUT`(details.task), finished는 즉시 `changed:false`. 탭 소실은 revision을 바꾸지 않으므로 대기 중일 때만 감지. 50ms UI tick은 대기가 있는 동안만 쓰며 대기 하나가 연결 worker 슬롯(16개) 하나를 제한 시간까지 차지한다. 슬롯 초과 연결은 응답 없이 끊겨 클라이언트는 `INCOMPLETE_FRAME`(outcomeUnknown)을 받고, 끊긴 클라이언트의 대기는 자기 제한 시간까지 남는다 |
| 네이티브 "AI에게 돌려주기" | paused 작업의 공간 카드와 해당 공간 도구 모음의 버튼(`ShellAction::ResumeSpace`). 렌더 시점의 revision으로 `task resume`과 같은 경로를 호출하며 오래된 revision은 `TASK_STALE_REVISION`으로 거부. 사용자의 현재 공간·선택은 바꾸지 않음. 중지 버튼 동작은 그대로 |
| `skill` | `docs/agent-cli-guide.md`를 `include_str!`로 내장해 출력. 앱 없이 동작하며 마지막 줄은 `[lightbrowser:skill] end of guide` |
| `wait TAB url PREFIX\|text SUBSTR [TIMEOUT_MS]` | 기존 탭 `wait`의 확장(`WaitUntil::Url{prefix}`, `Text{substring}`; 값은 1..4096바이트, NUL 불가). 같은 `Waiter`, 50ms 틱, 절대 기한을 쓰며 새 상주 타이머가 없다. `url`은 CDP 없이 네이티브 메인 프레임 URL(`frame.url()`)을 commit된 문서(`committed && !provisional`)에서 접두사 비교하며 기한과 같은 틱이면 일치가 우선한다. `text`는 `dom`과 같은 단일 in-flight 프로브(`getFrameTree`→격리 world→`Runtime.evaluate`)로 `dom_bridge.js`의 읽기 전용 `containsText`를 실행해 `found`일 때만 완료하고 `absent`·`loading`·예외·검사 실패는 프로브만 풀어 기한까지 재시도한다. 프로브 중 문서 epoch가 바뀌면 `PAGE_CHANGED`로 프로브만 버리고 새 문서를 다시 검사한다. 성공 응답에 `matched:"url"\|"text"`, 시간 초과는 `WAIT_TIMEOUT`(details.until/expected/navigationContinues). `task run` 소유권 검사와 `TASK_CONTROL_LOST`는 기존 wait와 같다 |
| `task list`, `task info ID` | 앱에 남아 있는 작업 상태 조회 |
| `task run ID REV COMMAND...` | 해당 작업의 현재 제어권으로 기존 명령 실행 |
| `task adopt ID REV TAB` | 기존 탭을 borrowed로 연결 |
| `task release ID REV TAB` | 탭을 닫지 않고 작업에서 분리 |
| `task handoff ID REV` | active 작업을 paused로 바꾸고 revision 증가 |
| `task resume ID REV` | paused 작업을 active로 바꾸고 revision 증가 |
| `task finish ID REV [--keep TAB...]` | revision을 증가시키고 작업 종료 시작 |

내 공간 ID는 `user`이며 작업 공간 ID는 작업 ID와 같다. 작업을 생성하면 탭 없는 공간이 목록에 생기지만 CEF 탭이나 다른 프로필은 만들지 않는다. 현재 보고 있는 공간도 바꾸지 않는다.

| 명령 | 표시 동작 |
| --- | --- |
| `spaces` | 모든 공간과 현재 공간, 선택 탭, overview 상태 조회 |
| `task run ID REV spaces` | 현재 revision을 검사하고 해당 작업의 공간 항목만 조회 |
| `space show user` | 내 공간과 그 공간에 저장된 선택 탭 표시 |
| `space show ID` | 해당 작업 공간과 그 공간에 저장된 선택 탭 표시 |
| `space overview` | 현재 공간의 선택을 보존한 채 공간 목록 표시 |

세 표시 명령의 응답에는 `spaces`, `activeSpace`, `activeTab`, `overview`가 있다. 공간 항목은 `id`, `name`, `kind`, `status`, `revision`, `tabs`, `selectedTab`, `active`를 가진다. `kind`는 `user` 또는 `task`, `status`는 `user`, `active`, `paused`, `finishing`, `finished` 중 하나다. 빈 공간의 `selectedTab`은 `null`이다. overview에서 페이지 뷰가 숨겨져도 `activeTab`은 저장된 선택을 나타낸다. CLI 메타데이터만으로 실제 표시나 포커스를 판단하지 않는다.

`space show`와 `space overview`는 사용자 화면을 바꾸는 명시 명령이며 창을 앞으로 가져온다. 공간을 보는 것만으로 AI 제어권을 인계하지 않는다. 두 명령을 `task run` 안에서 실행하면 `SPACE_UI_REQUIRES_UNSCOPED`로 거부한다. 작업 범위의 `spaces`는 목록 항목만 제한하며 최상위 현재 화면 메타데이터는 그대로 반환한다.

adopt와 release는 revision을 바꾸지 않는다. handoff, resume, finish의 첫 상태 전환은 revision을 하나 증가시킨다. finishing 또는 finished 상태에서는 현재 revision과 빈 keep 목록으로만 finish를 다시 호출할 수 있으며 revision은 유지된다. 이때 keep 탭을 추가하면 `INVALID_KEEP`로 거부한다. 작업 관리 명령을 task run 안에 중첩하지 않는다.

`task run ... tabs`는 그 작업이 소유한 탭만 반환한다. 범위를 지정하지 않은 `tabs`는 전체 탭과 작업 소속을 볼 수 있다. 작업 소속 탭을 범위 없이 읽거나 조작하면 `TASK_REQUIRED`가 발생한다. 다른 작업의 탭을 지정하면 `TASK_TAB_MISMATCH`, 이전 revision을 사용하면 `TASK_STALE_REVISION`, paused 작업에서 실행하면 `TASK_PAUSED`로 거부한다.

handoff는 CLI, 브라우저 상태 표시와 공간 카드의 중지 버튼으로 요청한다. 작업 소유 탭의 네이티브 주소창, 뒤로, 앞으로, 새로고침, 탭 닫기를 조작할 때도 먼저 소유 작업을 handoff한다. active 공간의 새 탭 버튼과 ⌘T도 해당 작업을 handoff한 뒤 사용자 탭을 같은 표시 공간에 만든다. 본문을 대상으로 하는 네이티브 클릭, 키 입력, 휠도 현재 표시와 포커스를 확인한 뒤 해당 작업을 handoff한다. 탭 선택 단축키는 이 관찰보다 먼저 처리하며 선택만으로 AI 작업을 중지하지 않는다.

handoff 시 해당 작업의 대기 요청은 `TASK_CONTROL_LOST`로 끝나며 기존 요소 참조도 만료된다. 단순 대기는 실행 결과가 불명확한 변경 작업이 아니다. 이미 페이지에 전달된 변경 작업은 오류 상세의 `outcomeUnknown`을 확인해야 하며 자동 재시도하면 안 된다. resume 뒤에는 현재 revision으로 다시 읽고 새 참조를 사용한다.

탭을 adopt하기 전에 진행 중인 요청이 없어야 한다. 작업으로 연결한 뒤에도 이미 실행 중인 읽기나 조작과 겹치면 `TAB_BUSY`로 거부할 수 있다. 이 규칙은 작업을 연결하기 전에 시작한 요청이 이후 제어권을 벗어나 완료되는 문제를 막는다.

active 작업의 탭에서 생성한 팝업은 해당 작업의 created 탭으로 연결한다. paused 상태에서 사용자가 여는 팝업은 작업 소속 없이 유지하지만 opener와 같은 표시 공간에 넣는다. 팝업 생성 요청과 실제 탭 생성 사이에도 작업 상태가 바뀔 수 있으므로 생성 콜백에서 제어권을 다시 확인해야 한다.

## 단일 브라우저 창

AppKit NSWindow 하나와 공유 콘텐츠 뷰 안에 CEF Alloy의 Chromium 페이지 뷰를 넣는다. 왼쪽에는 접을 수 있는 세로 탭, 위쪽에는 주소창과 탐색 버튼을 배치한다. 세로 탭 목록에는 현재 표시 공간의 탭만 보인다. 공간 목록에서는 상태와 대표 탭의 작은 미리보기를 가진 카드로 전환한다. 새 작업 창이나 UI용 웹 renderer를 만들지 않는다.

탭 행은 `ShellTab.failure`를 갱신 비교에 포함한다. 배경 탭의 실패만 바뀌어도 기존 행을 갱신하며 아이콘은 실패, 음소거, AI 제어, 로딩, 일반 상태 순서로 표시한다. 툴팁과 접근성 설명은 이 상태들을 함께 설명한다. raw crash 값 대신 `Navigation::page_failure`를 사용하므로 복구 중의 오래된 종료 기록을 계속 경고하지 않는다. 새 상태 캐시나 타이머를 추가하거나 탭을 자동 선택하지 않는다.

UI는 중성 배경과 청록색 선택 표시를 사용한다. 탐색 막대는 44pt, 탭 행은 36pt이며 주소 입력 영역과 작업 상태 배지를 분리한다. SF Symbols는 고정된 아이콘 종류만 공유 캐시에 보관하고, 아이콘 버튼에는 접근성 이름과 도움말을 유지한다. 공간 카드에는 미리보기, 제목과 탭 정보, 상태 배지와 중지를 모아 표시한다. 창 너비에 따라 1~3열로 배치하고 기존 미리보기 이미지를 비율 유지·상단 중앙 기준으로 잘라 보여준다. 별도 확대 이미지나 디스크 캐시는 만들지 않는다. 네이티브 표면은 창의 유효 appearance에 맞춰 라이트·다크 색상을 선택한다.

범위 없는 CLI open은 내 공간에 새 탭을 배치한다. 내 공간을 보고 있고 overview가 아닐 때만 새 탭을 선택한다. 작업 범위의 open은 해당 작업 공간에 넣되 어느 공간을 보고 있든 기존 선택을 유지한다. 선택이 없는 공간은 첫 탭을 대표 탭으로 기록한다. 다른 탭에 대한 read와 screenshot도 현재 선택을 바꾸지 않는다. 팝업은 실제 opener의 작업 소유권과 표시 공간을 검사한 뒤 같은 콘텐츠 영역에 붙인다. 소유권은 계속 CEF browser ID 단위로 유지한다.

`target="_blank"`처럼 CEF가 새 탭이나 창으로 분류한 링크 요청은 `OnOpenURLFromTab`에서 받아 UI 작업 큐에 전달한다. 큐 실행 시 opener가 남아 있는지, 작업 소속, 상태, revision이 요청 시점과 같은지 확인한 뒤 새 CEF 탭을 만들고 생성 콜백에서도 다시 검사한다. 제어권이 바뀌거나 생성이 실패하면 원래 탭을 대신 이동시키지 않는다. active 작업의 생성은 현재 선택을 유지한다. 사용자 전경 요청은 현재 보고 있는 같은 공간에서만 새 탭을 선택하며 `NEW_BACKGROUND_TAB`은 기존 선택을 유지한다.

페이지 우클릭의 `링크 주소를 새 탭에서 열기`도 기존 링크 큐와 생성 경로를 사용한다. 메뉴를 열 때 검증한 HTTP(S) URL을 고정하고 명령 직전에 페이지와 프레임, 제어권을 재검사한다. 필요한 handoff 이후 소유권과 원래 공간을 고정해 같은 공간의 사용자 배경 탭을 만든다. 정상 메뉴 종료와 단순 선택 변경은 승인된 URL 요청을 취소하지 않으며 큐와 생성 완료 시 원본 탭, 공간과 소유권이 달라지면 거절한다. 이 URL 기반 생성은 링크의 DOM click과 referrer, opener 연결을 재현하지 않는다. 세부 수명과 검증 한계는 [우클릭 메뉴 계약](context-menu-control.md)을 따른다.

CLI의 `tab` 값, 작업의 `tabs`, `closingTabs`, `releasedTabs`, 요소 참조와 탐색 상태의 관리 단위는 계속 CEF browser ID다. 탭 선택이나 표시 위치가 바뀌어도 살아 있는 탭의 ID는 바뀌지 않는다. `status.windowMode`는 `single`, `status.activeSpace`는 현재 공간 ID, `status.activeTab`은 그 공간의 선택 탭 ID다. `status.overview`는 공간 목록 표시 여부다. `tabs`의 `space`는 표시 공간이며 `task`는 소유권을 나타낸다. 각 탭의 `active`는 저장된 선택과 일치하는지 표시하며 선택 탭이 다른 작업에 속하면 작업 범위의 목록에는 `active:true`가 없을 수 있다.

현재 공간의 작업 이름과 상태, 중지 버튼을 표시한다. 공간 카드에도 개별 중지 버튼을 두며 탭 없는 active 공간도 중지할 수 있다. 중지는 화면에 표시한 작업 ID와 revision으로 기존 TaskHandoff 경로를 호출한다. 다른 작업을 일괄 중지하거나 페이지 로딩을 중지하는 버튼이 아니다. 이미 전달된 조작은 되돌리지 않는다. 재개와 작업 생성 등은 CLI에 남긴다. `lightbrowser panel`은 브라우저 창을 앞으로 가져오는 호환 명령이다.

AppKit 컨트롤은 UI 전용 renderer를 만들지 않는다. 변경 알림은 100ms 동안 모아 갱신하며 상주 polling 타이머는 없다. 탭 드래그의 가장자리 자동 스크롤은 동작 중에만 네이티브 타이머 하나를 사용하고 종료와 취소 때 해제한다. 숨은 탭의 포커스 요청은 취소한다. 닫기 승인 뒤 해당 자식 뷰만 해제하고 OnBeforeClose에서 탭 상태와 소유권을 정리한다. 마지막 탭까지 종료된 뒤 메시지 루프를 끝낸다. 창 전체 닫기도 각 브라우저에 정상 닫기를 요청해 beforeunload 처리를 거친다.

## 방문 기록 저장과 수집

`main → history_runtime::initialize`는 기존 데이터 폴더의 방문 기록 Store를 파일 worker에서 읽는다. `browser`의 탐색 콜백은 후보만 갱신하고 탭당 한 지연 작업에서 문서, 실제 주소, 탐색 세대, 공간과 소유권을 다시 확인한다. `history_runtime::record/update_title → Work::Change → history::Store::apply_updates`가 파일 작업을 직렬화하고 같은 URL의 대기를 합친다. 캐시 게시와 검색 UI 갱신은 완료 결과를 받은 뒤 수행한다.

네이티브 `control_panel`의 통합 검색은 `tab_search`에서 Tab, Bookmark, History를 구분하며 전체 40행 상한을 유지한다. History 선택은 현재 ID와 삭제 세대를 재확인한 뒤 `open_history_url → open_saved_user_url`로 사용자 탭을 연다. 신규 탭은 검증 성공 전까지 숨은 생성 상태를 유지하고 기록을 보류한다. 현재 주소 복사와 북마크 생성도 같은 기록 보류 경계를 사용한다.

후속 Query 결과는 검색 세대와 잘림 상태를 검사하고 `address::resolve_with_kind`에서 얻은 주소/검색 분류로 표시한다. 실제 사용자 선택 때 다시 해석한 URL을 `open_search_url → open_saved_user_url`에 전달한다. 새 탭 준비 전후에도 원래 검색과 화면을 검증하고 성공한 경우만 검색 화면을 닫는다. 한 행을 기존 40개 상한에 포함하며 입력 중 네트워크 요청은 없다. [검색 탐색 계약](search-navigation.md)을 따른다.

전체 삭제는 세대를 먼저 바꾸고 진행 중 쓰기 뒤에 실행한다. 예전 후보와 ID를 거절하고 새 탐색의 대기는 삭제 뒤에 쓴다. `main`의 정상 종료는 새 관찰을 닫고 제한된 대기를 완료한 다음 CEF를 종료한다. 파일 처리, 삭제 당시 문서 제외와 검증 한계는 [방문 기록 설계](history-design.md)에 있다. 소스와 단위 검사를 완료했으며 앱 번들 반영과 실제 CEF, 네이티브 검사는 남는다.

## 종료와 보존

finish는 작업이 만든 created 탭 중 keep 대상이 아닌 탭만 닫는다. `--keep`의 created 탭은 CLI 소유권을 해제한 뒤 완료 공간에 남긴다. borrowed 탭은 소유권을 해제하고 adopt 전의 표시 공간으로 돌려보낸다. 원래 공간이 다른 완료 공간이어도 그곳으로 복귀한다. 별도 release 명령도 borrowed 탭은 원래 공간으로, created 탭은 내 공간으로 옮긴다. 옮기는 탭을 따라 표시 공간을 자동 전환하지 않는다. 원래 공간의 선택 탭을 옮겼다면 남은 탭으로 선택을 보정한다. 목적지의 기존 선택은 유지하며 선택이 없을 때만 옮긴 탭을 선택한다.

완료 공간에 탭이 없으면 목록에서 제거하고 그 공간을 보고 있었다면 내 공간으로 돌아온다. 다만 다른 작업에 빌려준 탭의 복귀 목적지인 공간은 비어도 보존한다. 작업 기록 한도 때문에 finished 기록이 제거돼도 결과 탭이나 복귀 참조가 남은 표시 공간은 유지한다. 작업 소유권과 표시 공간의 수명이 서로 같지는 않다.

finish 응답의 `closeRequested`는 닫기 요청이 전달된 탭 목록이다. 실제 종료를 보장하는 값은 아니다. CEF 닫힘 콜백이 처리될 때까지 상태는 finishing이며 `closingTabs`에 남는다. 해당 탭이 실제로 닫힌 뒤 finished가 된다. 완료 여부는 task info와 현재 탭 목록으로 확인한다.

작업 기록은 앱 메모리에만 있다. 기록은 최대 128개, 작업당 탭은 생성 예약을 포함해 최대 4개이며 완료 기록을 정리해 한도를 유지한다. 앱이 종료되면 작업 ID와 revision은 사라진다. 작업 기록을 디스크에 저장하거나 프로필 데이터로 복원하지 않는다.

현재는 마지막 CEF 탭의 닫힘 콜백이 처리되면 앱과 소켓이 종료된다. 별도로 남는 작업 제어 창은 없다. finish가 마지막 탭을 닫았다면 후속 task info 조회는 불가능할 수 있다. 탭이 없는 빈 브라우저 창을 유지하는 수명주기는 구현하지 않았다.

렌더러의 예상치 못한 종료는 탭 닫힘과 구분한다. CEF 종료 콜백에서 해당 탭의 남은 DevTools 요청과 wait를 `RENDERER_TERMINATED`로 끝낸다. 네이티브 캡처 자리는 실제 결과, DevTools 연결 종료 또는 탭 닫힘까지 유지한다. 이미 전달한 변경 작업은 `outcomeUnknown:true`, 준비 단계와 읽기·대기는 `false`로 응답한다. 요소 참조와 문서 준비 상태는 무효화하지만 탭, 작업 소속, revision은 유지한다.

`tabs.renderer`는 `status`와 종료 진단용 `error`를 제공한다. `terminated`는 종료 후 복구 탐색이 진행 중이지 않은 상태, `recovering`은 명시적인 reload·navigate 등 새 탐색을 시작했지만 아직 새 문서가 commit되지 않은 상태다. commit 뒤에는 `running`으로 전환한다. 복구 탐색이 새 문서 없이 끝나면 `terminated`로 돌아가며 다시 시도할 수 있다. `running`은 DOM 준비나 응답성을 보장하지 않으므로 기존 wait 조건을 별도로 확인한다. 종료 당시 작업을 자동 재시도하지 않으며 복구 명령에도 기존 작업 소유권과 revision 검사를 적용한다.

## 메모리와 생성 예산

`src/resources.rs`는 아직 생성 콜백이 도착하지 않은 탭도 예약으로 집계한다. CLI open과 active 작업 팝업은 전체 기존 CEF 탭과 예약이 8개에 도달하면 거부한다. 작업 한도는 실제 소속 탭과 예약을 합쳐 4개다. 팝업의 생성 완료, 생성 취소, opener 종료에서 예약을 반환한다. 취소된 예약이나 만료된 제어권으로 늦게 생성된 탭은 조작 대상에서 제외하고 정상 종료를 요청한다.

active 작업의 새 탭 링크는 UI 큐에 넣기 전에 자동화 생성 자리를 예약하고 거절·취소·생성 실패 시 반환한다. 작업 소속이 없는 탭이나 paused 작업의 탭에서 여는 링크에는 이 자동화 생성 예산을 적용하지 않는다. handoff 중의 팝업이나 기존 사용자 탭을 한도 때문에 닫지 않으며, 이미 많은 사용자 탭이 있으면 추가 자동화 탭만 거절한다. borrowed와 keep 탭의 보존 규칙도 유지한다. 단일 창 안에서도 CEF browser ID마다 한 자리를 사용하며 탭 수 제한은 메모리 바이트 상한이 아니다.

`src/capture.rs`는 CLI 캡처 결과의 base64를 파일로 스트리밍 디코딩한다. 실제 캡처는 한 번에 하나만 허용한다. CEF가 시작 단계에서 실행을 거절하면 자리를 즉시 반환한다. 실행이 수락된 뒤에는 CLI 시간 초과, handoff, 렌더러 종료 알림이 와도 실제 결과, DevTools 연결 종료 또는 탭 닫힘 콜백까지 캡처 자리를 유지한다. `src/protocol.rs`는 JSON 직렬화 중 프레임 크기를 제한한다. 측정과 한계는 [메모리 관리](memory.md)를 참고한다.

공간 미리보기는 overview를 표시할 때 현재 카드의 대표 탭만 요청한다. 한 페이지의 카드와 캡처 후보는 최대 8개이며 다음 카드 페이지로 전환할 수 있다. 미리보기와 CLI screenshot은 같은 캡처 자리 하나를 공유한다. 반복 캡처 타이머와 디스크 캐시는 없으며 목록을 닫으면 카드와 UI 이미지 참조를 해제한다. 작은 이미지도 필요에 따라 디코딩 버퍼와 AppKit 이미지 메모리를 사용한다.

CEF clip은 최대 240×150 포인트를 목표로 축소 요청한다. 수신 PNG는 1MiB, 640×480 픽셀 이하인지 확인한 뒤 NSImage로 디코딩한다. 실제 네이티브 카드의 미리보기 표시는 확인했지만 PNG 픽셀 크기는 직접 측정하지 않았으므로 240×150 픽셀이라고 단정하지 않는다. 목록이 닫히거나 대표 탭, 탐색 세대, 공간 소속이 바뀐 뒤 도착한 결과는 버린다. 숨은 CEF 탭 자체의 페이지 실행이나 메모리를 폐기하는 기능은 아니다.

## 관찰과 입력

새로고침과 로딩 중지는 표시한 `NavigationControlTarget`을 런타임에 전달한다. CEF 로딩 상태를 다시 읽어도 의도를 뒤집지 않으며, 탭과 문서, 소유권 검증 뒤 필요한 작업만 기존 handoff 경로로 인계한다. 중지 중 주소 표시 보류는 호출 수명 안에만 유지하고 실제 문서 변화에는 초안 보존을 허용하지 않는다. [동작 계약과 검증 경계](reload-stop.md)를 따른다.

`check`는 CLI와 원본 JSON의 boolean을 `Command::Check`로 받고 기존 탭 소유권, 작업 revision과 탐색 epoch 검사 후 isolated DOM bridge에 전달한다. 요청 준비와 취소 경로에서는 페이지 변경 동작으로 분류한다. 실패 응답의 시도 여부와 현재 상태는 원래 요청 종류를 확인한 뒤 최대 네 boolean만 오류 상세로 전달한다. 기존 `fill`, `select`, `press`의 문자열은 타입을 보존한 JSON으로 직렬화한다.

체크박스와 라디오 활성화는 브라우저의 DOM 클릭 한 번에 맡기고 후속 상태를 확인한다. ref는 원래 문서와 읽기 세대를 보관하므로 동작 도중 새 read가 실행돼도 이전 참조로 후속 작업을 계속하지 않는다. 혼합 상태와 라디오 해제는 거절하며 실패 후 자동으로 다시 클릭하지 않는다. [체크 제어 계약](check-control.md)에 오류와 검증 수준을 기록한다.

번호 선택과 탭 순환은 공통 `select_tab_command`에서 `tab_selection_target`으로 대상을 구하고 `browser::select_tab_from_sidebar`를 호출한다. AppKit의 활성 창과 입력기 조합을 확인한 뒤 최신 화면에서 대상을 고르고, 런타임의 실제 소속과 수명 검사를 통과해야 선택한다. 순환은 현재 공간의 사이드바 순서를 따르며 한 탭뿐이거나 선택이 없으면 종료한다. 기존 범용 `run_action`의 선행 화면 닫기와 초안 삭제를 거치지 않고, 성공한 선택을 확인한 뒤에만 목록 화면을 닫고 포커스를 준다. 같은 선택 경로는 AI 소유권이나 revision을 변경하지 않는다. 실제 이벤트와 네이티브 포커스 검증은 소스 단위 검사와 구분한다.

네이티브 파일 메뉴의 `현재 주소를 내 공간 새 탭에서 열기`는 현재 보이는 탭의 식별자, 공간, 탐색 세대와 주소를 대상으로 잡는다. 런타임은 실제 main frame 주소와 상태를 재검증하고 사용자 탭을 배경에 생성한 뒤 성공 시 선택한다. 원본 AI 작업을 handoff하거나 페이지 문서를 복제하지 않는다. 생성 중 화면 또는 소유권이 바뀌면 오래된 선택을 실행하지 않고, 정리 대상은 이번에 만든 아직 소유되지 않은 사용자 탭으로 제한한다. 메뉴와 런타임 경계, 실제 검증의 남은 범위는 [동작 계약](open-current-page.md)을 따른다.

`open`과 탐색 명령은 요청 수락을 반환한다. `wait dom`은 현재 문서의 파싱 상태를, `wait load`는 전체 로딩 완료를, `wait url`은 네이티브 메인 프레임 주소 접두사를, `wait text`는 `read`와 같은 예산의 보이는 본문 텍스트 포함 여부를 확인한다. 네트워크 실패, 204 응답, 명시적 stop을 이전 문서의 준비 상태와 구분한다. 시간 초과는 탐색이나 탭을 자동으로 취소하지 않는다.

[CLI 상태 관측](cli-state-observation.md)은 native 값의 조회와 JSON 조립을 분리한다. 실제 main-frame URL을 유지하며 조회 전후의 Browser, 문서, 로딩 세대와 작업 권한을 확인한다. 탭 목록은 부분 결과를 성공으로 반환하지 않고 대기는 원래 마감 시간을 유지한다. 생성 후 조회 실패는 생성된 탭 ID를 명시하며, 대기 만료 때 상태 관측을 확정하지 못하면 `stateUnavailable:true`로 구분한다.

DOM 준비 확인의 최종 응답이 도착하면 해당 요청은 더 이상 진행 중인 probe가 아니다. 탐색 오류나 기한 만료로 결과를 채택하지 못할 때도 이 표시를 해제하며, 최종 실패 응답과 현재 탭 상태는 기존 대기 타이머가 반환한다. 다음 타이머 전에 새 탐색이 오류를 지운 경우 문서 상태를 다시 확인할 수 있어야 한다. 이 처리는 페이지 변경 작업의 재시도가 아니다.

read는 메인 문서와 열린 Shadow DOM의 본문, 조작 가능한 요소와 참조, documentId, readyState, viewport를 반환한다. 중첩된 열린 root와 슬롯으로 배치된 요소를 관찰하며 라벨은 요소가 속한 root에서 찾는다. 비밀번호 입력값과 숨김·inert 영역은 제외한다. 후속 소스는 접근 가능한 iframe과 그 안의 열린 Shadow DOM도 함께 읽는다. frame별 문맥과 접근 불가 상태는 `page.frames`로 구분한다. 기존 `read`는 접근 불가능한 iframe 내부를 순회하지 않으며 교차 출처 문서의 별도 조회는 아래 `read-frame` 경로를 사용한다. 닫힌 Shadow DOM과 캔버스 내부는 검사하지 않는다. 본문은 DOM과 열린 root의 순회 순서로 텍스트 노드를 정규화해 구성한다. 슬롯의 화면 순서나 원래 줄바꿈, 화면 전체·접근성 트리 전체를 재현하는 결과는 아니다.

본문 12,000자와 조작 요소 120개 외에 방문 노드 10,000개, 열린 shadow root 64개, 순회 깊이 64를 적용한다. 라벨 조회도 방문 예산을 사용하므로 예산이 소진되면 일부 라벨은 비어 있을 수 있다. `truncated.text`, `truncated.interactive`, `truncated.nodes`, `truncated.shadowRoots`, `truncated.depth`는 각 상한 때문에 관찰이 잘렸는지 표시한다. 관찰용 순회와 출력의 제한이므로 페이지 자체 DOM 크기나 Chromium 메모리를 제한하지 않는다.

참조는 읽기 세대와 네이티브 탐색 세대에 연결된다. 새로운 read, 페이지 이동, 방문 기록 복원, 제어권 넘김, 렌더러 종료 이후에는 새 참조가 필요하다. 조작 시 탐색 세대와 제어권을 확인하고, 대상 요소와 shadow host·slot 경로의 제거, 숨김, inert, 비활성화 여부를 검사한다. 화면이나 DOM 레이아웃의 모든 변화를 감지하는 것은 아니다.

후속 소스의 클릭은 대상과 부모 iframe을 스크롤하고 화면 좌표를 검사한 뒤 Chromium 포인터 입력을 요청한다. 포인터 이동 뒤 원래 참조의 같은 좌표를 다시 확인하며 지원하지 않는 프레임 변환은 거절한다. 키 입력은 원 요청과 준비 응답의 키를 대조한 뒤 keyDown 또는 rawKeyDown, keyUp을 요청하며 수동 form.requestSubmit은 실행하지 않는다. 기본 키 14개와 Shift 조합 10개에 macOS 편집 조합 18개를 더해 총 42개 형식을 지원한다. read는 반환 참조 범위에서 관찰한 focusedRef에 같은 탐색 세대 접두사를 붙이며 확인 불가이면 null로 둔다. 엔진 응답을 페이지 동작의 성공이나 물리 입력의 증거로 해석하지 않는다. [포인터 입력 설계](pointer-input.md)와 [키 입력 설계](keyboard-input.md)에 지원 범위와 경합을 기록한다. 텍스트 fill은 Chromium 입력과 값 확인으로 연결하며, contenteditable도 Chromium 입력과 렌더된 텍스트 확인을 사용하며, 다른 입력 유형과 select, check는 기존 DOM 동작을 유지한다. 실제 Naru CEF 통합과 배경 선택 보존은 미검증이며 지원 밖 프레임 좌표 변환, 닫힌 Shadow DOM, 캔버스 내부와 명시 목록 밖의 조합키는 미지원이다. 교차 출처 문서의 입력 7개는 아래 read-frame 참조 경로를 사용한다.

`fill`은 포커스 전과 실제 값 쓰기 직전에 동일한 지원 요소·입력 종류 규칙을 검사한다. 포커스 처리 중 체크박스, 파일 등 지원하지 않는 종류로 바뀌면 `ELEMENT_UNAVAILABLE`로 거절하고 fill의 값 쓰기와 input/change 이벤트를 실행하지 않는다. 계속 지원하는 text와 password 사이의 전환은 허용한다. 페이지의 포커스 처리 자체가 만든 변경을 되돌리는 기능은 아니다. 2026-10-15 보완의 상태 단위·모의 DOM 검사와 실제 CEF 미검증 범위는 작업 기록을 따른다.

후속 text, search, tel, url, password와 textarea는 포커스 뒤 입력 종류와 전체 선택 범위를 확인하고 Input.insertText 한 번으로 입력한다. Rust는 요청 문자열과 원 Browser, 문서, 소유권, isolated context를 요청 수명 동안 유지하며 준비, 입력, 사후 확인 사이에 같은 pending을 연결한다. 엔진 ACK만으로 완료하지 않고 원 context와 ref의 네이티브 value를 요청 문자열과 비교한다. 다른 지원 유형은 기존 setter와 합성 input, change 경로를 유지한다. 값 미도달과 대상 변경에 따른 미확정을 구분하며 입력 뒤 DOM fallback이나 자동 재시도는 없다. [일반 입력칸 계약](native-fill-control.md)에 동기 확인과 비동기 결과의 경계를 기록한다.

명시적 HTML `contenteditable` 영역의 fill도 전체 Range 선택과 Chromium Input.insertText를 사용한다. 준비에서 전체 선택과 원래 포커스를 검사하고, 엔진 응답 뒤 구조와 텍스트 예산을 확인한 후 네이티브 innerText를 요청 문자열과 비교한다. 합성 beforeinput과 직접 자식 교체는 제거했다. [contenteditable 계약](contenteditable-control.md)에 지원 범위와 편집기 호환성, 실제 앱 미검증 범위를 기록한다.

`insert`는 기존 FillTarget과 입력 배치를 재사용하되 포커스 전후 원래 값과 선택을 비교하고 선택 영역을 직접 바꾸지 않는다. Input.insertText 뒤 입력칸은 예상 값, contenteditable은 원래 host의 일치하는 input 이벤트를 확인한다. 원 Browser, 문서, 작업 제어권과 isolated context에 결합한 임시 관찰 token을 준비와 사후 확인에서 대조한다. DOM fallback이나 자동 재시도는 없으며 [삽입 계약](insert-text.md)에 상태 확인과 이벤트 관찰의 차이를 기록한다.

키 입력은 두 엔진 응답 뒤 같은 isolated context에서 짧게 유지한 keydown 대상 관찰을 소비한다. 다른 대상이 관찰되면 KEY_TARGET_MISMATCH이고, 탐색 등으로 관찰을 얻지 못하면 unconfirmed를 명시한다. PressTarget은 요청 키, 참조와 관찰 token을 묶고 pending 잠금은 사후 확인까지 이어진다. 페이지 관찰자는 최대 하나이며 새 동작이나 읽기, 소비, 단발 만료에서 정리한다. 사용자 이벤트를 막지 않으며 지연 포커스 변경 자체를 예방하는 장치는 아니다.

명령 전송과 raw 응답은 [DevTools 연결 경로](devtools-routing.md)에서 관리한다. 예약한 ID를 그대로 전송하고, 완료 처리 전에 원래 탭의 Browser와 정확한 session을 확인한다. `frames TAB`은 현재 탭의 iframe 후손만 지연 연결하고 각 renderer의 로컬 FrameTree를 수집한다. 수집 중에는 탐색·문서 세대, 소유권과 프레임 변화를 검사하며 단계 사이의 native getter 재진입에서도 수집 상태를 유지한다. 메타데이터 관찰은 원자적 snapshot이 아니다.

2026-10-18 소스는 `read-frame TAB FRAME`을 이 수집 경로에 연결한다. 요청마다 현재 프레임을 다시 찾고 원래 Browser·session·문서·작업 소유권을 유지한 채 해당 문서 하나의 본문과 요소 설명을 읽는다. 고정 이름 `LightBrowserFrameControl`의 isolated world를 재사용하며, `Runtime.disable` ACK 뒤 `Runtime.enable`로 생성 이벤트를 받는다. `ContextTracker`는 정확한 frame ID·world 이름·isolated 유형·isDefault=false에 해당하는 context 하나만 보관하고, `createIsolatedWorld`의 숫자 ID를 대조한 뒤 같은 session의 `Runtime.evaluate`에 `uniqueContextId`만 전달한다. context 교체·파괴·초기화나 프레임·제어권 변경은 이전 읽기를 무효화한다.

정상 읽기는 `Runtime.disable` ACK 뒤 결과를 반환한다. 실패·취소는 root Runtime이 활성화됐다면 그 해제와 root autoAttach 해제를 별도 ACK로 추적하고, 정리 대기나 실패가 남으면 새 프레임 수집을 막는다. `Runtime.disable`은 이벤트 보고를 끄는 정리이며 이미 시작한 JavaScript 실행 취소나 isolated world 해제를 보장하지 않는다. 정상 읽기 후 제한된 child session은 재사용할 수 있다.

후속 구현은 `readFrameActions`의 약한 DOM 참조에 native frame 접두사를 붙여 반환한다. 탭마다 최신 묶음 하나에 최대 16개 문서의 Browser·탐색 및 문서 epoch·소유권·frame/session·unique context·문서 ID·읽기 세대와 전체 최대 120개 ref를 묶는다. `supportedActions`는 click·hover·fill·insert·press·check·select이며 `pointerActionsSupported:true`로 반환한다. 접수되어 시작한 새 read-frame·read-page는 이전 묶음을 먼저 무효화하며 오래된 참조로 새 문서를 자동 재시도하지 않는다.

프레임 입력 전에 현재 tree를 다시 수집하고 최대 32단계의 부모를 확인한다. 각 부모의 별도 고정 world에서 frame owner를 resolve하고 같은 unique context의 함수 인자로 전달해 원문서 소속·연결·숨김·비활성을 검사한다. 임시 원격 객체는 objectGroup으로 추적하고 release ACK 후 다음 부모로 넘어간다. root 취소 정리는 객체 group·Runtime·autoAttach ACK를 각각 추적하며 child는 session detach로 정리한다. 대상 world는 원래 unique context와 일치해야 하며 `frameAct`는 DOM 바인딩을 먼저 확인한다. Chromium 입력 배치와 사후 확인까지 같은 snapshot/session/context와 요청 잠금을 유지한다. 입력 성공도 Runtime disable ACK 후 전송한다.

후속 포인터 경로는 로컬 요소와 부모별 content box·viewport를 교차해 clip을 구한 뒤 최대 9개 후보를 부모의 hit-test로 거른다. leaf의 특정 위치와 루트 전송 위치를 분리하며 mouseMoved 뒤 원 local point, 모든 부모와 같은 root point를 다시 확인한 다음 hover를 끝내거나 click 두 이벤트를 보낸다. 후보를 찾지 못하면 leaf scrollIntoView 한 번 후 전체 기하를 다시 측정하며 이동 후 스크롤·후보 재선택·입력 재시도는 없다. 각 부모의 객체를 순차 해제하고 원 target으로 돌아올 때 Runtime replay로 unique context를 재확인한다. `src/frame_pointer.rs`는 유한 수치·공유 clip·후보 부분집합을 검증한다. 이 순차 검사는 레이아웃과 입력을 원자적으로 묶지 않으며 지연 focus 경합도 예방하지 않는다. 회전·skew·perspective·pinch는 미지원이다. 단일 문서 상한과 read-page 공통 예산을 구분하며 약한 refs의 world별 메타데이터는 world 수명까지 남을 수 있다. [입력 계약](frame-input.md)과 [고정 엔진 조사](../artifacts/frame-input-v1-sources.json)에 범위를 기록한다. 실제 Naru CEF·외부 사이트·E2E는 실행하지 않았으며 최신 빌드 근거는 [체크리스트](ego-parity-e2e.md)를 따른다.

접근 가능한 iframe은 최대 16개 문서 슬롯과 중첩 8단계에서 contentDocument 조회 전에 제한한다. 기존 노드, 텍스트, 참조와 옵션 예산은 전체 문서가 공유한다. ref에는 원래 소속 문서와 부모 iframe 연결을 묶고 focus, keydown 뒤에도 문서 identity와 가용성을 확인한다. document별 isolated realm과 자원 경계, 미검증 범위는 [iframe 관찰과 입력](iframe-control.md)에 기록한다.

## JavaScript 대화상자

CEF `JsdialogHandler`(`src/dialog_runtime.rs`)가 `alert`, `confirm`, `prompt`와 `beforeunload`를 소유권으로 분기한다. 작업이 Active로 소유한 탭은 콜백을 UI 스레드의 대기열(`src/dialogs.rs`, 탭당 1개, 전체 4개)에 보관해 네이티브 대화상자를 만들지 않는다. 사용자 공간 탭과 중지된(사용자 제어) 작업 탭은 `false`를 반환해 CEF 기본 대화상자를 쓴다. 마무리 중인 작업 탭은 닫기가 멈추지 않도록 즉시 처리한다. 상세 계약과 한계는 [dialog-control.md](dialog-control.md)를 따른다.

대화상자가 열린 탭은 렌더러가 멈추므로 `handle`이 `Close`와 `Stop` 외의 탭 명령을 `DIALOG_PENDING`으로 즉시 거절하고, 대기열에 넣는 순간 그 탭의 진행 중 요청과 `wait`도 같은 코드로 끝낸다. 콜백 `cont`는 공유 상태나 AppKit 호출을 잡은 채 실행하지 않는다. 사이드바 행과 공간 카드는 `ShellTab.dialog_pending`으로 대기 표시를 그리며 공간이나 선택은 바꾸지 않는다.

## 임시 페이지 포커스와 입력 수명

키와 Chromium 텍스트 입력은 정확한 Browser·session의 임시 focus emulation을 사용한다. enable ACK 뒤 원 ref의 준비를 새로 실행하고 기존 입력 배치 한 번, disable ACK, 읽기 전용 결과 확인 순서로 진행한다. legacy DOM fill은 읽기 전용 사전 분류 뒤 이 설정 없이 실행하며 분류한 유형과 경로를 고정한다. 별도 네이티브 창 포커스·공간 선택 호출을 추가하지 않는다.

취소가 일반 pending을 제거해도 임시 설정 해제 책임은 별도 상태에 남는다. 해제 미확정 상태에서는 같은 탭의 새 페이지 조작을 거절하고 원 Browser·session·ID의 늦은 응답을 구분한다. 입력을 자동 재전송하지 않는다. 일반 입력칸의 fill 관찰은 insert와 단일 임시 receipt를 공유하며 beforeinput·input의 다른 대상 증거와 최종 값을 구분한다. 자세한 상한·정리와 비원자적 입력의 한계는 [포커스 준비](focus-input.md)와 [입력칸 계약](native-fill-control.md)을 따른다.

## 여러 문서 통합 읽기

`read-page`는 `start_frame_inventory`에서 프레임 연결을 수집한 뒤 루트부터 너비 우선으로 최대 16개 문서를 읽는다. 각 문서는 재귀 없는 `readFrameActions(remaining)`을 사용하며 `src/page_read.rs`가 본문 UTF-16 길이, 반환 요소·옵션·메타데이터 개수와 보고된 순회 사용량을 검증하고 차감한다. 본문 읽기 전에 확인한 PAGE_LOADING만 사용량 0인 개별 누락으로 처리한다. 다른 오류나 context 변경은 전체 요청을 실패시킨다.

완료한 문서의 context tracker도 전체 읽기가 끝날 때까지 유지한다. 마지막 Runtime disable ACK와 현재성, 전체 응답 2MiB 검사를 통과한 뒤에만 참조 묶음을 게시한다. 응답 본문은 전송 뒤 native snapshot에 보관하지 않는다. 약한 DOM Map과 isolated world를 즉시 지우는 기능은 아니며 실제 누적 메모리는 E2E에서 확인한다. [통합 읽기 계약](page-read.md)에 상한과 비원자적 관찰의 한계를 기록한다.

## UI와 AI의 후속 설계

단일 창의 네이티브 탭 UI는 Rust의 작업 상태와 명령 경로를 사용한다. 후속 UI 기능도 CLI와 같은 소유권 규칙을 유지해야 한다. UI만 작업 상태를 변경하는 별도 경로를 만들지 않는다.

`src/mac.rs::send_event`는 브라우저 단축키를 먼저 처리하고, 소비하지 않은 이벤트를 `observe_page_input`으로 넘긴 뒤 AppKit에 전달한다. 본문의 클릭, 키 입력, 휠은 `handoff_page_input`으로 현재 공간과 소유 작업을 다시 확인한다. 편집 메뉴는 `observe_page_menu_action`에서 같은 경로를 사용한다. 페이지의 합성 DOM 이벤트를 이 네이티브 사용자 개입으로 취급하지 않으며, OS 자동 입력이 실제 장치 입력임을 증명하는 경로도 아니다. 실제 IME와 나머지 편집 메뉴, 입력 중 대기 요청 취소 검증은 제품 계획의 후속 범위다.

CEF의 페이지 우클릭 메뉴는 `itemSelected:`로 기본 명령을 실행하므로 일반 편집 selector 검사와 별도로 `ContextMenuHandler`에서 다시 검증한다. 탭에 메뉴 대상과 세대를 보관하며 실행 직전 선택, 메인 문서, 프레임과 소유권을 확인한다. native getter와 기존 handoff 뒤에도 재검사하고 모든 프레임 browse와 load-start에서 메뉴를 무효화한다. 오래된 명령은 기본 실행을 소비하며 동일 페이지의 상태 표시줄로 재열기를 안내한다. 네이티브 객체를 추가 보관하거나 기본 메뉴를 새 UI로 대체하지 않는다. 고정 CEF의 콜백 순서와 실제 앱 미검증 범위는 [메뉴 제어권 계약](context-menu-control.md)을 따른다.

내장 AI 채팅과 모델 호출은 아직 없다. 모델을 연결하더라도 관찰, 판단, 실행을 구분한다. 웹페이지 본문을 실행 지시로 취급하지 않는다. 로그인한 페이지에서 어떤 정보를 모델로 보낼지, 외부 전송이나 중요한 작업을 언제 확인받을지는 모델 연결 단계에서 명시한다. CLI 위에 MCP 어댑터를 붙일 수 있지만 앱이나 작업 관리의 필수 구성으로 두지 않는다.

## 검증과 배포 순서

현재 개발은 남은 기능을 먼저 구현하고 통합 빌드 후 [14개 E2E 사용 여정](ego-parity-e2e.md)으로 확인한다. 아래 스크립트는 각 여정의 실행 수단이며 기능마다 전체 회귀 묶음을 반복하는 절차가 아니다.

1. `tests/tasks_smoke.py`로 소유권, revision, handoff·resume, 종료 보존을 확인하고 브라우저 안의 작업 중지 버튼이 같은 경로를 사용하는지 검사한다. 기존 탐색·자원 제한 테스트도 유지한다.
2. `tests/shadow_smoke.py`로 열린 Shadow DOM의 관찰·입력·참조·순회 상한을 검사한다. 렌더러 종료의 요청 취소와 명시적 복구는 별도 상태 전환으로 확인한다. 이후 관찰 범위나 실제 입력 이벤트를 확장할 때도 제어권 변경 중 취소를 검사한다.
3. `tests/single_window_smoke.py`로 명시 공간 전환 후 선택 메타데이터, 배경 관찰과 캡처를 확인한다. `tests/spaces_smoke.py`로 작업 생성과 open의 선택 보존, 공간별 선택 복원, 빈 공간, adopt와 release의 복귀, 완료 결과 보존을 확인한다. 네이티브 창 수, 카드와 미리보기, 사이드바 접기, 실제 포커스, 팝업과 입력 handoff는 별도로 검사한다.
4. 페이지 정보 전송 정책과 사용자 확인 흐름을 정한 뒤 모델 연결을 추가한다.
5. 지속 서명, 공증, 배포, 업데이트와 CEF 갱신 절차를 검증한다.

단위 테스트는 상태 전환과 파서의 근거다. 로컬 CEF 통합 테스트는 해당 명령으로 관찰한 CEF 탭, DOM, 소켓 동작의 근거다. 단일 창 테스트의 CLI 메타데이터 검증을 네이티브 창 수나 UI 버튼 클릭 검증으로 해석하지 않는다. 둘 다 외부 사이트의 전체 호환성이나 배포 완료를 뜻하지 않는다. 작업 공간, Shadow DOM과 단일 창 테스트는 로컬 HTTP fixture와 직접 만든 탭만 사용하며 기존 탭과 프로필을 삭제하지 않는다. 위 절차는 검증 범위를 설명하며 실행 결과는 별도 기록한다.

선택한 E2E 스크립트는 이미 실행 중인 최종 앱과 같은 데이터 디렉터리를 지정해 실행한다. 기존 탭 1~5개와 추가 생성·캡처가 진행 중이지 않은 상태가 필요하며, 다른 CLI 테스트와 동시에 실행하지 않는다. PNG는 새 파일로 남기고 JSON은 표준 출력으로 반환한다.

```bash
python3 tests/single_window_smoke.py --data-dir "$LIGHT_BROWSER_DATA_DIR" \
  --output-dir artifacts/single-window
python3 tests/spaces_smoke.py --data-dir "$LIGHT_BROWSER_DATA_DIR"
```

공간 테스트는 기존 사용자 탭을 포함해 초기 탭 1~4개가 필요하다. 자체 로컬 HTTP 페이지와 탭만 만들고 종료 시 생성 예약과 캡처 반환을 검사한다. 메타데이터에서 선택이 유지된다는 결과를 실제 AppKit 포커스나 미리보기 이미지 표시의 검증으로 대체하지 않는다. 공간 UI v3의 자동 회귀와 네이티브 검사 결과는 아래에 구분해 기록하며 이전 단일 창 v2 기록과 합산하지 않는다.

CLI의 합성 DOM 클릭을 실제 사용자 입력으로 간주하지 않는다. 새 탭 링크와 `window.open` 동작은 네이티브 UI에서 검사하며, 새 탭 생성·원래 문서 보존·실제 opener 작업 소속·선택 상태를 CLI로 다시 확인한다. 단일 창 자동 테스트는 합성 클릭의 팝업 생성 여부를 성공 조건으로 사용하지 않는다.

개발 빌드는 새 번들의 서명을 검증한 뒤 해당 프로젝트 경로의 이전 앱을 종료하고 교체한다. 이전 번들과 Cargo 중간 산출물을 정리하고 최신 실행 파일 세 개를 남긴다. 사용자 프로필은 빌드 정리 대상이 아니다. 재시작 테스트는 자신이 만든 프로필만 성공 후 정리하고 JSON, 로그, PNG 증거를 보존한다.

현재 ad hoc 서명은 코드가 달라지면 키체인 접근 재승인이 필요할 수 있다. `scripts/dev.sh`는 기본적으로 빌드만 수행하며, `--run`을 지정해도 명시적 non-ad-hoc 서명자가 없으면 빌드 전에 중단한다. 이 실행 분리는 키체인 권한 유지의 해결이 아니다. 안정적인 개발 서명은 반복 승인 문제를 해소하기 위한 미완료 검증 항목이며, 배포용 서명과 공증도 별도로 남아 있다. CEF 156 beta와 로컬 파생 바인딩 역시 업데이트 시 함께 점검해야 한다.

## 공간 UI v3 검증 기록

2026-10-10 최종 바이너리의 SHA-256은 `710ec71ecfcb0fa374d875eba0ad24492b3a6d63d603e8a0da455bac0d7a8cca`다. 키체인에서 사용자가 항상 허용한 뒤 동일 바이너리의 로컬 HTTP 페이지 로딩과 검증을 완료했다. 재서명 후 승인 없이 실행할 수 있다는 근거는 아니다.

| 검증 | 확인한 범위 | 근거와 제한 |
| --- | --- | --- |
| Rust 단위 88개 | 상태 모델, 프로토콜, 제어권과 자원 처리 | `artifacts/spaces-v3-rust-tests.log`, 실제 UI 동작과 구분 |
| 모의 DOM 20개와 Python 격리 22개 | DOM 처리와 개발 스크립트의 격리 검사 | `artifacts/spaces-v1-dom-tests.log`, `artifacts/spaces-v1-python-tests.log`, 앱 바이너리 실행 검사가 아님 |
| 로컬 CEF 자동 회귀 54항목 | 기존 7개 묶음 44항목과 새 공간 10항목, 총 8개 묶음 통과 | `artifacts/spaces-v3-suite.json`, `artifacts/spaces-v3-spaces-smoke-after-allow.json` |
| 공간 자동 검사 10항목 | 배경 생성과 읽기의 선택 보존, 명시 공간 전환, 빈 공간, scoped UI 전환 거부, 완료 결과 유지, borrowed 복귀와 정리 | 위 공간 보고서, 기존 사용자 탭 보존, 최종 생성 예약과 캡처 0 |
| 네이티브 수기 16항목 | 빈 카드, 작업 중지, 사이드바, 단축키, 실제 로컬 페이지와 미리보기, 팝업, 선택 복원, 정리 | `artifacts/spaces-v3-native.json`, 정리 확인도 항목 수에 포함 |

네이티브 검사에서 내 공간의 페이지를 그대로 표시한 채 두 작업의 로컬 페이지를 CLI로 읽었다. 공간 목록에는 서로 다른 세 페이지의 미리보기가 표시됐고 배경 작업 탭의 탐색 뒤 제목과 이미지가 갱신됐다. 원래 overview와 다른 카드도 유지했다.

실제 페이지 버튼의 `window.open`은 같은 표시 공간의 task 소유 탭을 만들었으며 opener의 URL과 선택은 바꾸지 않았다. 새 탭을 직접 선택한 뒤 내 공간으로 갔다가 돌아오면 그 탭을 다시 표시했다. 빈 작업과 페이지가 있는 작업의 중지는 해당 작업을 paused, revision 2로 바꾸고 이전 revision 요청을 거절했다. 다른 작업은 active, revision 1을 유지했다.

⌘⇧S로 공간 목록 열기, ⌘⇧B로 사이드바 접기와 복원, ⌘T로 사용자 새 탭 생성과 선택, ⌘L로 주소창 선택과 문자열 변경, ⌘W로 테스트 탭만 닫기를 확인했다. 주소창에서 탐색은 제출하지 않았으며 초기 사용자 탭은 보존했다. 정리 뒤 자체 로컬 페이지와 작업 공간을 제거하고 생성 예약과 캡처가 0인지 확인했다.

미리보기 PNG의 실제 픽셀 크기와 디코딩 이미지 할당량, 8개 카드와 여러 목록 페이지의 부하, 현재 버전의 마지막 탭 종료는 검증하지 않았다. 페이지 본문 직접 입력의 자동 handoff도 아직 없다. 이 결과는 외부 사이트 검증, GitHub CI, 배포나 업데이트 검증이 아니며 이전 단일 창 버전의 마지막 탭 종료 결과를 현재 바이너리에 적용하지 않는다.

## 공간 분리 이전의 단일 창 검증 기록

2026-10-10 공간 분리 이전 단일 창 v2 바이너리의 SHA-256은 `a8aef7f3d125f75855912469e6e5b2aa4574d8fbe793c2953af3371dc99ab265`다. `artifacts/single-window-v2-validation.json`에서 Rust 단위 테스트 75개, Python 격리 테스트 22개, 모의 DOM 테스트 20개와 실제 CEF를 이용한 로컬 HTTP 회귀 7개 묶음, 총 44개 항목 통과를 확인했다. 이 결과는 새 공간 모델과 네이티브 UI의 검증이 아니다.

| 검증 | 확인한 범위 | 근거와 제한 |
| --- | --- | --- |
| 자동 회귀 44항목 | 입력·캡처 7, 탐색 준비 6, 기본 조작 7, 작업 공간 6, 자원 제한 7, Shadow DOM 5, 단일 창 6 | `artifacts/single-window-v2-validation.json`과 개별 보고서; 로컬 HTTP·CLI 검사 |
| 단일 창 자동 검사 | 선택 메타데이터, 배경 읽기·캡처, panel 호환, 다른 작업·기존 탭 보존, 생성 예약·캡처 0 | `artifacts/single-window-v2-single_window_smoke.json`; 네이티브 창 수·팝업·중지 UI는 별도 검사 |
| 같은 바이너리의 네이티브 UI | 실제 새 탭 링크가 opener URL·선택을 보존, 새 탭도 같은 창·작업에 연결, 탭 선택 후 주소·페이지 표시, 작업 중지 뒤 paused·revision 1→2와 이전 revision 거부 | `artifacts/single-window-v2-native.json`; 합성 클릭의 팝업 생성 여부로 대체하지 않음 |
| 같은 바이너리의 단축키·종료 | ⌘T 새 탭, ⌘L 주소창 포커스·문자열 변경, 초기 탭만 닫아도 다른 탭 유지, ⌘W로 마지막 탭을 닫은 뒤 프로세스 exit 0·PID 부재 | 같은 네이티브 보고서; ⌘L 검사에서 탐색하지 않았으며 모든 종료 경합을 검증한 것은 아님 |

배경 캡처는 같은 실행의 전경 A와 픽셀을 비교하고 전경 B와 구별했다. CSS 색상값이 PNG에 그대로 저장된다고 가정하지 않는다. 새 탭 링크와 작업 중지는 실제 네이티브 클릭 후 CLI로 상태를 다시 확인했다. 이 바이너리에서는 beforeunload 취소 성공을 주장하지 않는다.

이전 빌드·실패 근거는 `artifacts/single-window-final-result.json`, `artifacts/single-window-release-validation.json`, `artifacts/single-window-native-before-menu.json` 등에 보존하며 위 항목 수에 합산하지 않는다. 외부 사이트 전체 호환성이나 배포 완료를 뜻하지 않으며, 메모리 측정과 비교 한계는 [메모리 관리](memory.md)에 별도로 기록했다.
