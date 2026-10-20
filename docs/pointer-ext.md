# 포인터 입력 확장과 선택자 대상

[클릭과 마우스 올리기](pointer-input.md)의 Chromium 포인터 경로(DevTools `Input.dispatchMouseEvent`, 이동 뒤 같은 위치 재검사, 임시 페이지 포커스, 작업 소유권·revision 검사)를 그대로 쓰는 명령 확장이다. 새 렌더러, 타이머, 저장 파일을 만들지 않는다. 사용 방법은 [에이전트 CLI 가이드](agent-cli-guide.md)의 "선택자와 포인터 확장"을 따른다.

## 명령

| 명령 | 입력 순서 | 성공 응답의 `action` |
| --- | --- | --- |
| `click TAB TARGET [--button left\|right]` | 왼쪽은 기존 click. 오른쪽은 mousePressed·mouseReleased(button `right`, buttons 2) | `click`(오른쪽은 `button:"right"`) |
| `dblclick TAB TARGET` | 누름·해제 두 쌍, clickCount 1 다음 2 | `dblclick`(`clickCount:2`) |
| `wheel TAB TARGET DX DY` | 이동·확인 뒤 `mouseWheel` 하나(deltaX, deltaY) | `wheel` |
| `drag TAB FROM TO` | 이동·확인, 누름, 10단계 mouseMoved(button left, buttons 1, 마지막이 TO), 해제 | `drag` |
| `click-at TAB X Y [--button left\|right] [--count 1..3]` | 이동·확인 뒤 누름·해제 쌍을 `--count`번(clickCount 1..N) | `click-at` |
| `move-at TAB X Y` | mouseMoved 하나와 확인 | `move-at` |

모든 명령은 `task run` 안에서 같은 문법으로 쓴다. 프로토콜 이름은 `dblclick`, `right_click`, `wheel`, `drag`, `click_at`, `move_at`이다. 기존 `click` JSON(`ref`만)은 바뀌지 않았다. 왼쪽 단일 클릭의 입력 JSON과 응답 모양도 기존과 같다.

## 공통 경로

1. 준비: 브리지가 ref의 문서·표시·비활성 상태를 확인하고 기존 규칙으로 hit test 지점을 고른다(`click`과 `dblclick`, 오른쪽 클릭은 같은 `click` 준비 동작, `wheel`은 `relaxed`).
2. `mouseMoved`를 보내고 응답 뒤 같은 지점을 스크롤 없이 다시 검사한다. 실패하면 이후 입력을 보내지 않는다.
3. 마지막 입력을 보낸다. 입력 배치는 입력 종류가 정한 개수의 메시지를 예약하고 하나씩 응답을 추적한다. 취소·소유권·문서 변경은 매 메시지 직전에 다시 검사한다.
4. 자동 재시도, 자동 release, 대상 교체는 없다. 중간 실패 뒤에는 눌린 버튼이 남을 수 있다.

`wheel`은 중앙 아래에 다른 상호작용 요소(목록 안의 링크)가 있어도 허용한다(`relaxed`). 포인터가 대상의 후손 위에 있기만 하면 스크롤 대상이 같기 때문이다. `scroll`은 DOM `scrollBy`이고 `wheel`은 트러스트된 wheel 이벤트다. 서로 대체하지 않는다.

## 오른쪽 클릭과 네이티브 메뉴

Chromium은 오른쪽 입력 뒤 페이지에서 `contextmenu`가 취소되지 않으면 CEF를 통해 메뉴 실행기를 시작한다. AI 탭의 CLI 입력이 사용자 창 위에 네이티브 메뉴를 열면 안 되므로 [우클릭 메뉴 계약](context-menu-control.md)의 보호를 확장한다.

- `dispatch_input`이 오른쪽 누름 직전에 탭의 보호 기한(`State.context_click_until`)을 현재 시각 + 5초로 기록한다. 기한은 한 번만 쓴다: `RunContextMenu`가 메뉴를 막으면 그 탭의 기록을 지워, 이후 사용자의 우클릭(handoff 뒤)은 정상으로 열린다.
- 기한이 지난 뒤 늦게 도착하는 메뉴 요청도 상태로 막는다(`native_menu_blocked`). 탭의 소유 작업이 Active이거나, 탭이 활성 공간의 선택 탭이 아니거나, 공간 개요가 열려 있으면 `OnBeforeContextMenu`는 메뉴를 비우고 `RunContextMenu`는 `callback.cancel()` 뒤 true를 반환한다. 사용자가 직접 우클릭하면 기존 입력 경로가 먼저 작업을 handoff(Paused)하므로 사용자 메뉴는 영향받지 않는다.
- 막힌 요청에서는 네이티브 메뉴 실행기와 `OnContextMenuCommand`가 호출되지 않는다. 페이지의 `contextmenu` 이벤트와 `preventDefault`는 그대로 동작한다.
- 이 기한은 오른쪽 클릭에만 적용한다. `click-at --button right`도 같은 기한을 쓴다.

## 선택자

`text=SUBSTR`, `css=SELECTOR`, `role=ROLE[name=NAME]`을 REF 자리에서 모두 받는다. 접두사는 ref와 겹치지 않는다(ref는 `숫자/`나 `frame/`으로 시작).

- 해석: DOM bridge의 `resolve` 동작이 `read`와 같은 함수로 페이지를 한 번 읽고(본문 12000단위, 노드 10000, 열린 shadow root 64, 상호작용 요소 120, 접근 가능한 same-origin iframe), 반환된 상호작용 요소 중에서만 고른다. 페이지 DOM은 바꾸지 않는다. `read`가 보여 주는 `label`·`role`이 판단 기준이라 에이전트가 보는 것과 일치한다.
- `text=`: 정규화한 `label`에 부분 문자열(대소문자 무시). 따옴표로 감싸면 정확히 일치(대소문자 구분). `css=`: 요소의 `matches()`. 문법 오류는 `INVALID_SELECTOR`. `role=`: `role` 일치, 선택적으로 `[name=…]` 정확히, `[name*=…]` 부분(대소문자 무시).
- 결과: 정확히 하나면 계속, 0개는 `TARGET_NOT_FOUND`(`matchCount:0`, 읽기 예산이 잘렸으면 `truncated:true`), 2개 이상은 `TARGET_AMBIGUOUS`(`matchCount`와 후보 ref 최대 5개). 어느 경우도 입력은 보내지 않고 `attempted:false`, `outcomeUnknown:false`다.
- 계속: Rust가 결과 ref의 형식(`문서ID:세대:번호`)을 검증해 선택자를 `epoch/ref`로 바꾸고 같은 `handle`을 호출한다. 그 뒤는 ref 경로이므로 hit test, 숨김 탭 포커스, 소유권 검사, `STALE_REF` 처리가 같다. 성공 응답에 `resolved:[{selector,ref,role,label}]`을 붙인다.
- 비용: 선택자 한 번은 읽기 한 번이다. 읽기가 ref 번호를 새로 매기므로 이전 `read`의 ref는 만료된다. 그래서 `drag`는 FROM, TO가 둘 다 ref이거나 둘 다 선택자여야 하며 한 번의 읽기로 둘을 함께 구한다. 선택자 안의 두 대상이 같은 요소이면 `INVALID_ARGUMENTS`다.
- 범위: 메인 문서와 접근 가능한 same-origin iframe, 열린 shadow root. 교차 출처 프레임은 `read-frame`의 `frame/` ref를 쓴다(선택자 불가). 비상호작용 요소는 `css=`로도 고를 수 없다.
- 검증: 길이 1024바이트 이하, NUL 없음, `=` 뒤 비어 있지 않음. CLI는 종료 코드 2, 프로토콜은 `INVALID_REQUEST`.

## drag의 한계

- 마우스 이벤트(`mousedown`/`mousemove`/`mouseup`) 기반 끌기만 지원한다. HTML5 끌어놓기(`dragstart`)나 네이티브 파일 끌기는 합성 CDP 마우스 입력으로는 macOS의 끌기 세션을 안전하게 끝낼 수 없어 FROM이 `draggable="true"`, `<img>`, `<a href>` 안이면 `DRAG_UNSUPPORTED`로 거절한다(입력 없음). 선택된 텍스트 위에서 누르면 브라우저가 텍스트 끌기를 시작할 수 있으므로 이 경우는 막지 못한다.
- 두 지점은 한 viewport에서 함께 hit test를 통과해야 한다. 필요하면 점이 없는 쪽만 한 번씩 스크롤해 다시 확인하고, 그래도 둘이 함께 보이지 않으면 `CLICK_POINT_UNAVAILABLE`이다. 누르기 전에 두 점을 원래 위치로 다시 검사하며, 끄는 동안 또는 놓기 전에는 확인하지 않는다.
- 이동은 응답 하나마다 이벤트 하나를 보낸다(`paced`). 연속으로 보내면 Chromium이 mouseMoved를 합쳐 페이지가 중간 이동을 보지 못하기 때문이다. 각 이동은 엔진 응답 왕복을 포함하므로 지연이 생긴다.
- 중단 정리: 누름이 받아들여진 뒤 drag가 실패·취소·마감(task handoff, 작업 취소, 대화상자, 소유권 상실)으로 끝나면 왼쪽 버튼이 눌린 채 남지 않도록 마지막 좌표에서 `mouseReleased`(button left, buttons 0)를 정확히 한 번 보낸다(`PointerHold`). 소유권 검사는 적용하지 않지만(포커스 정리와 같은 규칙) 같은 Browser이고 렌더러가 종료되지 않았을 때만 보낸다. 대화상자가 대기 중이면 보내지 않고 `dialog`로 응답한 뒤 보낸다. 실패 응답에는 `details.pointerCleanup`(`released`: 엔진이 요청을 받음, `pending`: 대화상자 때문에 보류, `failed`: 보낼 수 없음)이 붙는다. 작업 취소처럼 응답이 이미 나간 경우에는 진행 중이던 이벤트의 늦은 응답이 도착할 때 정리한다. 렌더러가 응답하지 않으면 정리도 일어나지 않는다. 대화상자 보류는 `dialog` 명령 응답 뒤에 풀리며 handoff·release 등 다른 경로로 대화상자가 정리된 경우는 같은 탭의 다음 drag 정리 전까지 보류로 남을 수 있다.
- `frame/` ref를 쓴 `drag`는 `UNSUPPORTED_TARGET`이다. `dblclick`, 오른쪽 클릭, `wheel`은 교차 출처 프레임 포인터 경로의 마지막 입력만 바뀌어 지원한다.

## 좌표 명령

`click-at`, `move-at`은 root viewport의 정수 CSS 픽셀이다. 브리지의 `pointAt` 준비가 `0 <= x < width`, `0 <= y < height`(스크롤바 제외한 클라이언트 크기와 `innerWidth/innerHeight` 중 작은 쪽)를 확인한다. 밖이면 `INVALID_POINT`(입력 없음). 확대·visualViewport 오프셋은 기존 포인터와 같이 `UNSUPPORTED_GEOMETRY`다. 요소가 없으므로 hit test, 스크롤, 대상 확인이 없다. 이동 뒤 검사는 같은 좌표가 같은 viewport 안인지 다시 보는 것뿐이다. 좌표 아래의 요소가 그 사이에 바뀌었는지는 알 수 없다. 교차 출처 iframe 위의 좌표도 root 좌표로 그대로 전달한다.

## 검증 상태

실행 결과와 실행하지 못한 검사는 [작업 기록](work-log.md)의 2026-10-21 M4 항목을 따른다. 요약: Rust 단위(프로토콜·CLI·입력 배치·선택자 재작성·오른쪽 클릭 보호 기한)와 모의 DOM(선택자·drag·pointAt·relaxed wheel)과 로컬 CEF(`tests/pointer_ext_smoke.py`: 페이지가 센 이벤트 수·순서·trusted, 좌표, 선택자, 교차 출처 프레임 ref)를 실행했다. 네이티브 AppKit에서 컨텍스트 메뉴가 실제로 열리지 않는지, 실제 사용자 우클릭과 보호 기한의 상호작용, HTML5 끌어놓기, 외부 사이트는 확인하지 못했다.
