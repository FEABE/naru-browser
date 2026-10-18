# 브라우저 명령과 프레임 연결

2026-10-18 저장된 소스의 계약이다. 후속 `read-page TAB`은 여러 frame 문서를 공통 예산으로 읽고 최신 참조 묶음을 한 번에 공개한다. 이 변경의 [v9 통합 빌드](../artifacts/ego-parity-v9-build.json)는 성공했으며 실제 앱·CEF·E2E는 미실행이다. 아래 v7·v8 빌드 결과와 구분한다. 같은 탭의 `frames TAB` 메타데이터 조회와 `read-frame TAB FRAME` 읽기에 더해, 최신 frame ref의 `click`, `hover`, `fill`, `insert`, `press`, `check`, `select`를 연결했다. DOM 평가에는 원래 frame의 정확한 session/context를 쓰고, 프레임 포인터는 부모 좌표와 가림을 검사한 뒤 root session으로 보낸다. 이전 다섯 입력 연결의 [v7 release 빌드](../artifacts/ego-parity-v7-build.json)는 성공했다. 포인터 연결의 [v8 통합 빌드](../artifacts/ego-parity-v8-build.json)도 성공했으며 실제 CEF 실행은 미검증이다.

## 현재 명령 경로

`src/browser.rs`의 화면 미리보기, 입력 배치, 입력 후 확인, 로딩 대기와 일반 명령은 같은 raw DevTools 전송 함수를 쓴다. 이미 예약한 양의 ID를 JSON envelope에 넣고 CEF의 `SendDevToolsMessage`로 보낸다. 기존 `ExecuteDevToolsMethod`가 ID를 재할당할 가능성을 제거한다. 전송 API의 성공은 전달 접수이며 페이지 효과는 기존 사후 관찰로 구분한다.

`src/devtools_wire.rs`는 응답의 ID, 연결, result와 error 구성을 검사한다. ID가 양의 i32이고 result 또는 error 객체가 정확히 하나여야 한다. 필드 순서는 의미에 영향을 주지 않는다. 알려진 필드의 중복, 이벤트와 응답의 혼합, 잘못된 JSON은 거절한다. 루트 요청에는 sessionId가 없는 응답만, child 요청에는 예약한 sessionId와 정확히 같은 응답만 허용한다. sessionId 값이 null이거나 잘못된 형식인 경우도 루트로 간주하지 않는다.

응답을 소비하기 전에 ID와 session이 가리키는 탭을 찾고 콜백의 실제 Browser를 비교한다. 다른 Browser나 session의 응답은 원래 요청과 캡처 자리를 제거하지 않는다. 취소된 screenshot의 캡처 자리는 기존처럼 실제 응답이나 연결 종료까지 유지하며, 늦은 응답도 같은 Browser 확인을 거친다. 모든 응답은 raw 콜백에서 소비하므로 session 정보를 버리는 CEF typed callback으로 다시 전달하지 않는다. 연결 종료도 현재 Browser를 확인한 뒤 해당 탭을 취소한다.

요청 ID는 오버플로를 검사하며 재사용하지 않는다. 상한에 도달하면 새 명령을 거절하고, 미리보기는 새 캡처를 예약하지 않는다. 일반 요청의 취소 token, 작업 revision과 탭 잠금, 입력 배치의 재진입 보호와 전송 전후 검사는 유지한다. frame action은 아래의 snapshot·부모 검사·원래 context 검증을 추가로 사용한다. 이 가드도 브라우저 입력과 DOM 상태를 원자적으로 묶지는 않는다.

## `frames TAB` 조회

`frames TAB`과 `task run ID REV frames TAB`은 지정한 탭의 프레임 메타데이터를 조회한다. 작업 범위에서는 기존 task revision과 탭 소유권 검사를 거친다. 탭이나 공간을 선택하거나 전역 target을 가져오지 않는다. 연결 대상은 해당 Browser에서 발견된 `iframe`뿐이며 worker, 다른 페이지와 다른 작업의 탭은 등록하지 않는다.

첫 조회는 루트 session에 `Page.enable` → `Target.setAutoAttach` → `Page.getFrameTree`를 순서대로 보낸다. auto-attach는 `flatten:true`, `waitForDebuggerOnStart:false`와 iframe 필터를 사용한다. 발견한 각 child에도 같은 준비를 하고 해당 session의 FrameTree를 수집한다. 준비가 끝난 연결은 다음 조회에서 재사용하며, 결과용 프레임 목록과 방문 집합은 매 요청마다 새로 만들고 요청 종료 후 해제한다.

응답에는 `tab`, `epoch`, `documentEpoch`, `frames`, `truncated`, `atomic:false`, `contentIncluded:false`가 있다. 각 프레임 항목은 `frameId`, `parentId`, `sessionId`, `url`, `name`을 담으며 루트 session은 `sessionId:null`이다. `contentIncluded:false`는 이 응답에 문서 내용이 없다는 뜻이며 DOM 접근 가능 여부를 판정한 값이 아니다. URL과 이름은 제한된 메타데이터이고 문서 본문이나 actionable ref가 아니다. 기존 `read`의 `page.frames`와 식별자 계약도 다르다.

고정 Chromium 구현의 `Page.getFrameTree`는 해당 session의 로컬 프레임만 반환하며 RemoteFrame인 OOPIF 경계 노드 자체를 제외한다. child 결과의 루트에는 다른 session에 있는 부모의 frame ID가 들어갈 수 있다. 안정된 topology에서는 각 session의 로컬 subtree가 분리되므로 동일 frame ID가 다시 나타나면 앞선 값을 덮어쓰지 않고 `FRAME_CHANGED`로 수집을 종료한다. CEF Frame ID, targetId, URL이나 이름을 CDP frameId의 대체값으로 사용하지 않는다.

## `read-frame TAB FRAME` 읽기와 입력

`frames` 응답에서 얻은 CDP frame ID로 한 문서를 읽는다. 명령은 현재 탭의 inventory를 다시 수집해 대상과 session을 확정하며, 이전 응답의 session이나 URL을 그대로 실행 대상으로 사용하지 않는다. 기존 task 범위와 `TAB_BUSY` 경계를 유지하고 사용자의 공간·선택을 바꾸지 않는다. 상세 반환 형식과 제한은 [프레임 본문 읽기](frame-read.md)를 따른다.

확정한 session에서 `Runtime.disable` ACK → `Runtime.enable` → 고정 이름 `LightBrowserFrameControl`의 `Page.createIsolatedWorld` → `Runtime.evaluate` → `Runtime.disable` ACK 순서로 진행한다. 이미 enabled인 Runtime은 기존 context를 replay하지 않으므로 최초 disable로 상태를 맞춘다. enable 전에 tracker를 준비하고, 해당 session의 context 이벤트에서 frame ID·world 이름·`type:isolated`·`isDefault:false`, numeric ID와 unique ID를 확인한다. world 생성 응답의 numeric ID가 이 이벤트와 일치해야 `uniqueContextId`로 평가한다. 같은 session의 이벤트 replay와 새 context 통지는 응답 ACK 전에 flush된다는 고정 엔진 근거를 사용한다.

본문과 요소 메타데이터를 받은 뒤에도 대상 session의 Runtime disable ACK까지 성공해야 결과를 반환한다. DOM은 `readFrameActions()`로 최대 120개 약한 요소 참조를 만들고, Rust는 documentId/generation과 반환 ref를 검사해 요청별 접두어를 붙인다. focusedRef도 반환 집합의 실제 ref만 허용한다. 응답은 `atomic:false`, `supportedActions:["click","hover","fill","insert","press","check","select"]`, `pointerActionsSupported:true`다. 이는 입력 경로가 있다는 표시이며 모든 요소의 기하 조건을 보증하지 않는다. 현재 문서와 열린 Shadow DOM만 읽으며 자식 본문은 순회하지 않는다. 읽기 자체에는 부모 표시 검사나 최상위 포인터 좌표 검증이 없다.

Browser마다 최신 frame 읽기 묶음 하나를 유지한다. `read-frame`은 snapshot 하나, `read-page`는 최대 16개 문서 binding과 합계 120개 ref를 같은 묶음에 게시한다. 기존 CLI action은 그 안에 등록된 ref로 frame snapshot을 선택하고, 새 inventory에서 원래 frame·session을 확인한다. DOM 준비와 사후 검사는 snapshot의 원래 unique context와 documentId/generation binding으로 실행한다. 지원 fill·insert·press의 Chromium 입력도 같은 session을 사용하며 check·select와 나머지 fill 유형은 기존 DOM 경로를 유지한다. 원래 context가 없어지면 새 문서로 이어가지 않고 실패한다. click·hover의 DOM 대상도 같은 binding을 유지하지만 마우스 전송 session은 root로 분리한다.

입력 전에 target의 부모부터 root까지 실제 frame 부모 연결을 최대 32개 확인한다. 각 parent session에서 Runtime reset/enable → 고정 `LightBrowserFrameAncestors` world → `DOM.getFrameOwner(childFrameId)` → `DOM.resolveNode(parent numeric context, objectGroup)` → `Runtime.callFunctionOn(parent uniqueContextId, owner objectId 인수)` → group release ACK → Runtime disable ACK를 순서대로 실행한다. unique context와 다른 world의 인수는 V8이 실행 전에 거절하며, DOM 검사는 ownerDocument·연결·IFRAME/FRAME 종류와 composed ancestor의 숨김·inert·비활성·0 크기를 확인한다. 요청별 group은 Resolve 전에 기록하고 함수 호출의 예외 객체에도 같은 group을 지정한다. 부모 관계가 누락되거나 바뀌면 입력을 시작하지 않는다.

위의 비포인터 부모 표시는 샘플이며 clipping·포인터 가림 검사가 아니다. fill·insert·press의 Input 메서드도 특정 element/frame을 인수로 고정하지 않고 widget의 현재 포커스를 사용한다. 지연 focus handler와 동시 사람 입력 경합은 남으며, 사후 확인은 대상 불일치를 보고할 뿐 이미 생긴 효과를 취소하지 않는다. [프레임 입력 근거](../artifacts/frame-input-v1-sources.json)를 따른다.

## `read-page TAB`의 공통 예산과 게시

`read-page`는 새 inventory의 root부터 너비 우선으로 최대 16개 문서를 시도한다. 각 session tree에서 관찰한 형제 순서이며 전체 DOM·화면 순서는 보장하지 않는다. 각 문서는 자식 본문을 재귀 순회하지 않고, 남은 text·elements·nodes·roots·options·frames 예산으로 로컬 문서와 열린 Shadow DOM을 읽는다. 반환은 `documents:[{frame,page}]`, `unread:[{frame,reason}]`, 전체 `frames`와 `budget:{limit,used,remaining}`, `truncated`, `atomic:false`를 포함한다. 상세 한도와 누락 사유는 [여러 프레임 읽기](page-read.md)를 따른다.

bridge 실행 전 readyState/body 검사에서 확인한 문서별 `PAGE_LOADING`만 사용량 0의 unread로 남기고 다음 문서로 진행한다. 다른 context·frame·owner 변경이나 정리 실패는 일부 참조를 공개하지 않고 전체 요청을 실패시킨다. active token과 탭 잠금, 이미 읽은 문서들의 context 검사를 마지막 Runtime disable ACK까지 유지한 뒤 결과와 ref 묶음을 함께 게시한다.

새 `read-page` 또는 `read-frame`이 inventory 준비 단계에 수락되면 이전 묶음을 무효화한다. 그 뒤 실패해도 이전 ref를 복구하지 않지만 시작 전 진단의 `PAGE_LOADING`·`TAB_BUSY` 거절은 이전 묶음을 보존한다. 한 묶음의 서로 다른 frame ref는 각각 원래 문서 binding을 사용한다.

## frame click·hover의 분리된 좌표와 전송

`FramePointer`는 먼저 원래 leaf context에서 요소 rect와 viewport를 얻고, 각 부모에서 `inspectFramePointer`로 owner content 영역의 양수 축 정렬 mapping과 viewport를 수집한다. `src/frame_pointer.rs`는 이 영역들의 교집합을 leaf 좌표로 되돌려 후보 생성에 넘긴다. leaf는 이 영역에서 최대 9개 후보를 hit test하고, 부모별 새 조회는 실제 owner에 닿는 후보만 부모 좌표로 변환해 남긴다. Rust도 후보 index·중복·변환 결과·content/viewport 범위를 검사한다. 빈 clip이나 살아남은 후보가 없으면 포인터를 보내기 전에 leaf만 한 번 `scrollIntoView({behavior:'instant',block:'center',inline:'center'})`하고 leaf와 모든 부모 geometry부터 다시 측정한다. 최초와 재측정의 후보 생성은 합계 최대 18개다. 이동 후에는 스크롤하거나 다른 후보를 고르지 않는다.

선택한 leaf CSS point와 root CSS point를 별도로 보관한다. `Pending.session()`과 실제 입력 전송은 포인터 배치에서만 root session을 사용하고, DOM prepare/verify는 계속 원래 child session의 unique context를 사용한다. root `mouseMoved` ACK 후 leaf 동일점 → 모든 부모의 현재 owner hit test·좌표 변환 → 원래 root point와의 정확한 일치 → leaf 동일점 순서로 재검사한다. hover는 여기서 끝나고, click은 같은 root point의 mousePressed/mouseReleased를 기존 배치로 보낸다. 부모 검사마다 objectGroup release ACK와 Runtime disable ACK를 거치며, 명령의 마지막 대상 Runtime disable ACK까지 기존 탭 잠금을 유지한다.

일반 browser zoom과 device scale은 엔진이 처리하므로 CSS point에 devicePixelRatio를 다시 곱하지 않는다. iframe border/padding과 지원되는 양수 축 정렬 CSS scale/zoom을 반영하지만 회전·skew·perspective·motion path·SVG 조상, 설명할 수 없는 크기 차이, 확대나 offset이 있는 visualViewport는 거절한다. leaf 스크롤의 OOPIF 부모 전파는 비동기이며, 응답이나 재검사가 compositor 상태를 원자적으로 고정하지 않는다. 자세한 단계와 한계는 [프레임 포인터](frame-pointer.md), 기존 ref와 공통 결과 의미는 [포인터 입력](pointer-input.md), 고정 엔진 근거는 [소스 조사](../artifacts/frame-pointer-v1-sources.json)를 따른다.

## 수명과 정리

키·Chromium 텍스트 입력은 [임시 포커스 준비](focus-input.md)의 정확한 session 응답도 같은 Browser·ID 경계에서 처리한다. 일반 pending이 취소돼도 별도 임시 상태가 enable·disable 책임을 유지한다. 설정 해제 ACK 전에는 해당 child 연결의 추적 정리를 보류하며, 성공 경로는 disable ACK 뒤 읽기 전용 사후 확인과 기존 Runtime 정리를 진행한다. legacy DOM fill은 읽기 전용 분류 뒤 임시 설정 없이 실행한다.


수집은 원래 Browser, 메인 navigation epoch·document epoch, 탭 소유자와 상태, task scope, 활성 요청 token에 묶인다. 문서가 준비되지 않았거나 다른 페이지 명령이 진행 중이면 시작하지 않는다. CEF host 조회가 끝난 뒤에도 상태를 다시 검사하고, 응답 처리 전에 같은 검사를 수행한다. 이 검사는 배경 AI 작업 때문에 사용자의 선택이나 공간을 바꾸지 않는다.

`Target.attachedToTarget`, `Target.detachedFromTarget`과 `Page.frameAttached`, `Page.frameDetached`, `Page.frameNavigated`를 현재 Browser와 등록된 부모 session에 한정해 처리한다. 이미 읽은 session이나 현재 FrameTree를 변경하는 이벤트는 수집을 무효화한다. `FrameControl.active_inventory`가 단계 사이에도 현재 session·단계와 무효화 상태를 유지하므로, 앞선 pending을 제거한 뒤 CEF host 조회에 재진입해 생긴 변화도 다음 전송 전에 확인한다. 성공과 추적 중지 때 이 수집 상태를 해제한다. detach는 부모 관계가 맞는 session과 그 자손을 제거한다. 준비 중의 정상 attach는 수집 대상에 포함하며, 같은 연결의 중복 attach로 기존 항목을 교체하지 않는다.

본문 읽기와 입력 준비에서는 `Runtime.executionContextCreated`, `executionContextDestroyed`, `executionContextsCleared`도 추적한다. 이벤트 session과 일치하는 대상 tracker, 모든 부모 tracker와 이미 수집한 문서의 tracker에 전달하므로, 부모 여러 개가 같은 session을 쓰거나 다음 부모 검사 중 앞선 world가 바뀌어도 현재 부모 하나만 확인하지 않는다. 고정 world에 상충하는 context가 보고되거나 관찰한 context가 파괴·초기화되면 흐름을 무효화하며 다른 context로 자동 재시도하지 않는다. `uniqueContextId`는 numeric ID의 프로세스 간 재사용을 피하지만 기존 Browser·문서·소유권 검사를 대신하지 않는다.

조회·입력의 실패·취소·timeout, 제어권 변경과 renderer 종료 등에서 내부 `stop_frame_tracking`을 호출하면 snapshot을 무효화하고 로컬 session 목록을 비운다. root owner object group이 남았으면 release, root Runtime을 활성화했을 가능성이 있으면 disable, 그리고 root `Target.setAutoAttach(autoAttach:false)`를 각각 보낸다. 최대 세 정리 ID는 정확한 Browser의 개별 ACK까지 보관한다. child의 객체와 Runtime은 session 폐기로 정리하지만 Browser 연결에 남는 root 객체·Runtime에는 독립 ACK가 필요하다. 전송 접수만으로 해제 완료라고 판단하지 않으며, 정리 중이거나 정리에 실패하면 새 discovery를 거절한다. 연결 종료와 탭 닫힘에서는 해당 추적 상태도 제거한다. 이 내부 정리 함수는 CLI `stop TAB`의 탐색 중지 명령과 구분한다.

고정 Chromium에서는 root auto-attach 해제가 자손 session을 재귀 폐기하지만, 부모 라우팅을 먼저 제거해 자손별 detach 알림이나 진행 중 요청별 오류 ACK가 외부에 모두 전달되지는 않을 수 있다. 따라서 자손별 응답을 기다려 로컬 정리를 완료하는 구조가 아니다. [프레임 연결 소스 조사](../artifacts/frame-session-v1-sources.json)에 ACK 순서, 재귀 해제와 미검증 경계를 기록했다.

`Runtime.disable`은 context reporting과 일부 Inspector 설정을 정리하며 JS context를 파괴하거나 진행 중 evaluate를 취소하지 않는다. 정리 ACK를 평가 취소나 world 메모리 해제의 근거로 사용하지 않는다. 고정된 대상·부모 world는 재사용한다. 현재 CLI의 `readFrameActions()`는 WeakRef 요소와 WeakMap 문맥을 사용하며 같은 world의 다음 read에서 교체한다. 내부 content-only `readFrame()`의 finally 해제와 다르다. Rust가 최신 snapshot을 버려도 이전 frame world의 약한 Map과 bridge까지 즉시 지우는 것은 아니다. Runtime disable은 remote object group도 해제하지 않으므로 부모 owner group은 별도 release가 필요하다. [context 소스 조사](../artifacts/frame-read-v1-sources.json)에 근거를 남겼다.

## 메모리와 검증 경계

요청은 기존 params를 빌려 바로 바이트로 직렬화한다. 중간 CEF Dictionary와 params의 별도 JSON 복제를 만들지 않는다. 응답도 원래 콜백 바이트에서 result 또는 error를 빌려 사용하며 envelope 전체를 Value 트리로 만들거나 본문을 재직렬화하지 않는다. FrameTree 본문을 읽는 JSON 파싱과 기존 screenshot 디코딩까지 제거한 것은 아니다.

| 대상 | 상한과 수명 |
| --- | --- |
| child session registry | Browser당 최대 16개, session 깊이 8. 첫 조회부터 지연 유지하며 내부 추적 중지·연결 종료·탭 닫힘 때 정리 |
| 조회 결과 | 전체 최대 128개 프레임. 프레임별 새 예산이 아니며 요청 종료 후 해제 |
| FrameTree 정규화 | 한 tree에서 최대 128개 항목 방문, 루트 포함 깊이 32. 잘못된 자식 항목도 방문 예산에 포함 |
| 원본 응답·이벤트 | 본문 Read 단계 응답 2 MiB, 그 밖의 프레임 단계 응답 256 KiB, 관심 이벤트 32 KiB. JSON 파싱 전 상한 초과는 조회 실패 |
| 메타데이터 | ID 256 bytes, URL 4,096 bytes, 이름 256 bytes. 문자열 잘림과 누락·프레임 예산 제한은 `truncated`로 표시 |
| 읽기 예산 | 본문 12,000 UTF-16 units, 요소 120개, 노드 방문 10,000회, 열린 Shadow DOM 64개, 옵션 240개, 로컬 iframe 메타데이터 16개. read-page는 전체 문서가 공유하며 순회 깊이 64는 문서별 적용 |
| 읽기 임시 상태 | read-frame은 결과 JSON 하나, read-page는 최대 16번 문서 시도와 수집한 문서별 context tracker. 문서 JSON 합계와 최종 응답 각각 2 MiB이며 마지막 Runtime disable ACK 뒤 결과 게시·요청 종료 |
| 최신 frame 읽기 묶음 | Browser당 묶음 하나, 최대 16개 문서 binding과 합계 120개 ref. 본문 사본은 보관하지 않으며 DOM 요소는 WeakRef |
| 부모 검사 | 최대 32개 FrameRecord/context tracker, 순차 처리 중 owner 객체와 group 한 쌍. 정상 release ACK 또는 실패 시 root release/child detach로 정리 |
| 프레임 포인터 | 부모별 geometry 최대 32개, 원본·변환 후보 목록 각각 최대 9개, leaf/root point 한 쌍. 요청 동안만 보관하며 스크롤 재측정 때 기존 후보·geometry를 비움 |

연결 한도를 넘거나 연결 관계가 잘못되면 일부 session을 임의로 대체하지 않고 추적을 중지한다. 정상 조회 후 재사용하는 session registry와 요청마다 해제하는 결과는 구분한다. 새 상주 프로세스, UI renderer, worker, 저장 파일과 반복 타이머는 없다. 이 상한은 Chromium의 전체 프레임·renderer 메모리 상한이 아니며 실제 RAM 차이는 측정하지 않았다.

CEF 156의 typed parser는 sessionId를 전달하는 계약이 없으며 정해진 필드 순서도 가정한다. 표준 flat child 응답이 누락될 수 있다는 판단은 고정 CEF 소스에서 얻었다. 현재 Naru에서 잘못된 다른 탭 조작을 재현한 결과로 설명하지 않는다. [조회 기록](../artifacts/devtools-session-v1-sources.json)에 원본 경로와 검증 한계를 남겼다.

이번 문서 갱신은 현재 소스와 고정 upstream 구현을 읽어 정리한 것이다. 이전 metadata-only inventory의 [v5 빌드](../artifacts/ego-parity-v5-build.json), content-only 읽기의 [v6 통합 빌드](../artifacts/ego-parity-v6-build.json), 다섯 입력 연결의 [v7 빌드](../artifacts/ego-parity-v7-build.json)와 이번 포인터 변경을 구분한다. v7은 exit 0과 소스 34개의 전후 해시 일치를 확인했다. 포인터 연결의 [v8 통합 빌드](../artifacts/ego-parity-v8-build.json)는 exit 0, 91.49초로 완료했고 제품 소스·Cargo 35개의 전후 해시가 일치했다. 후속 read-page 변경의 [v9 통합 빌드](../artifacts/ego-parity-v9-build.json)는 exit 0, 95.05초이며 제품 소스·Cargo 36개의 전후 해시가 일치했다. 이번 단위·모의 DOM·앱·실제 CEF E2E는 실행하지 않았다. 컴파일이나 원문 조사로 실제 입력 성공을 대신하지 않는다.

## 남은 프레임 구현과 검증

1. frame ref의 click·hover 연결은 구현했지만 mixed-process 중첩, 부분 노출·부모 가림, 스크롤·zoom 및 hover 후 배치의 실제 CEF 동작은 미검증이다. 지원하지 않는 기하 조건은 계속 거절한다.
2. read-page의 공통 예산과 프레임별 로컬 읽기는 구현했다. 문서 중복 방지, 예산 합산, 일부 loading 처리와 여러 frame ref의 실제 입력 연계는 E2E가 필요하며 전체 DOM·시각적 읽기 순서 통합은 제공하지 않는다.
3. widget 포커스 기반 native 입력의 지연 focus·사람 입력 경합은 남는다. 원래 context 사후 검사가 원치 않은 입력을 예방·복구하는 보장은 아니다.
4. cross-origin/OOPIF의 read→일곱 action, hidden/inert 부모 거절, 오래된 ref와 process swap, 제어권 인계·취소, object group 정리 및 배경 사용자 선택 유지를 통합 E2E로 확인해야 한다.

SOP를 해제하거나 universal access를 허용하지 않는다. [통합 E2E 08](ego-parity-e2e.md) 확인 전에는 교차 출처 제어 전체나 ego lite와의 동등한 사용성이 완료됐다고 표시하지 않는다.
