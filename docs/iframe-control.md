# iframe의 CLI 관찰과 입력

2026-10-15에 시작한 구현이다. 최신 통합 후보의 반영 범위와 실제 CEF 실행 여부는 [E2E 목록](ego-parity-e2e.md)을 따른다. 사용자 선택으로 탭 복원과 세션 JSON은 보류했으며 이번 기능은 저장소를 추가하지 않는다.

## 범위와 결과

기존 `read`, `click`, `hover`, `fill`, `insert`, `select`, `press` 호출 형식을 유지하면서 메인 문서에서 접근 가능한 iframe 문서와 그 안의 열린 Shadow DOM을 읽는다. `interactive`는 기존 평면 배열이며 프레임 내부 항목에만 `frame` ID를 추가한다. 기존 native 탐색 세대 접두어와 task revision 검사는 그대로 적용된다.

`page.frames`는 해당 read에서만 유효한 `id`, `parent`, `label`, `status`를 반환한다. 부모는 `main` 또는 다른 프레임 ID다. 접근에 성공한 문서만 실제 `url`과 `title`을 제한 길이로 추가한다. status는 `accessible`, `unavailable`, `limit`다. inaccessible이나 아직 준비되지 않은 문서를 비어 있는 본문으로 추정하지 않는다. 선언된 src나 URL 문자열 비교로 실제 접근 권한을 판정하지 않고 `contentDocument`의 현재 결과를 사용한다. 다른 출처, sandbox의 opaque origin, RemoteFrame 등은 null이나 접근 예외가 될 수 있으므로 모두 cross-origin이라고 단정하지 않는다.

상한은 프레임 메타데이터 16개, 프레임 중첩 8단계다. unavailable과 깊이 limit 항목도 16개에 포함한다. 프레임 상한은 contentDocument 조회 전에 확인한다. 기존 본문 12,000자, 요소 120개, 노드 방문 10,000개, 열린 shadow root 64개, 순회 깊이 64와 옵션 전체 240개 예산은 문서마다 새로 부여하지 않고 전체 read가 공유한다. 잘림은 `truncated.frames`, `truncated.frameDepth`와 기존 필드로 구분한다. 라벨 스캔은 iframe의 본문으로 넘어가지 않는다.

## 참조 수명과 입력

프레임 참조는 요소와 소속 문서, 부모 iframe부터 메인 문서까지의 연결에 묶인다. action 전에 각 iframe이 원래 부모 문서에 연결돼 있고 현재 contentDocument가 기록한 문서와 같은지 확인한다. 새 탐색, 제거, 다른 문서로 이동한 요소와 접근 불가 전환은 오래된 참조로 거절한다. iframe과 상위 요소의 숨김, inert, aria-hidden, 비활성 상태 및 iframe의 0 크기도 검사한다. DOM 준비 도중 문서가 바뀌면 오래된 대상으로 후속 동작을 준비하지 않는다. Chromium 입력을 이미 전달한 뒤 대상이나 문서가 바뀌면 attempted와 결과 불확실성을 구분한다. 엔진 접수 뒤 발생한 효과를 되돌리거나 모든 늦은 이벤트를 차단했다고 설명하지 않는다.

스타일, Range, native form setter와 Event 생성자는 요소가 속한 문서의 realm을 사용한다. 클릭과 hover, 지원 키와 일반 텍스트 입력은 후속 Chromium 경로를 사용하며 나머지 입력 유형과 select, check는 DOM 경로다. [포인터](pointer-input.md), [키](keyboard-input.md), [삽입](insert-text.md) 계약의 결과 의미를 따른다. Chromium 이벤트를 실제 하드웨어 입력과 같다고 설명하지 않는다. 클릭이나 입력 자체가 페이지 탐색을 시작할 수 있고 이벤트를 전달한 뒤 페이지가 값을 바꿀 수 있으므로 실제 효과는 새 read로 확인해야 한다. 입력 결과를 무조건 성공한 업무 처리로 설명하지 않는다.

기존 `read`와 scroll 명령의 viewport·scroll은 메인 창 기준을 유지한다. 이 경로의 포인터 준비는 접근 가능한 부모 iframe의 좌표와 가림을 확인하며 지원하지 않는 transform은 거절한다. iframe 전용 스크롤 명령, 요소와 screenshot의 정합 보장, 닫힌 Shadow DOM과 캔버스는 지원하지 않는다. 교차 출처 문서는 아래의 별도 `read-frame` 또는 공통 예산의 `read-page` 경로로 읽는다. 그 최신 ref는 일곱 action에서 사용할 수 있지만 기존 read의 ref와 수명·라우팅이 다르다. frame 포인터는 leaf와 부모를 각자 검증한 뒤 root 좌표로 전달한다. CEF `grantUniveralAccess:false`를 유지한다.

## 엔진 근거와 자원

설치 Chromium 156.0.8078.17 소스에서 DOMWindow::Wrap은 현재 ScriptState의 World로 자식 WindowProxy를 선택하고 contentDocument 바인딩도 같은 World ID로 자식 문서를 감싼다. 프레임별 context와 realm은 별도다. [DOMWindow](https://github.com/chromium/chromium/blob/156.0.8078.17/third_party/blink/renderer/core/frame/dom_window.cc#L248), [문서 바인딩](https://github.com/chromium/chromium/blob/156.0.8078.17/third_party/blink/renderer/bindings/scripts/bind_gen/interface.py#L1667), [context 생성](https://github.com/chromium/chromium/blob/156.0.8078.17/third_party/blink/renderer/bindings/core/v8/local_window_proxy.cc#L321).

contentDocument의 반환값은 DOM 보안 출처 검사를 거치며 접근할 수 없으면 null이다. same security origin이어도 RemoteFrame인 경우 접근할 수 없다. [보안 검사](https://github.com/chromium/chromium/blob/156.0.8078.17/third_party/blink/renderer/bindings/core/v8/binding_security.cc#L79), [null 반환 바인딩](https://github.com/chromium/chromium/blob/156.0.8078.17/third_party/blink/renderer/bindings/scripts/bind_gen/interface.py#L995).

자식 WindowProxy 접근은 해당 isolated context를 지연 생성할 수 있다. 프레임 수와 깊이 제한은 한 read가 접근하는 문서와 응답을 제한한다. 반복 read에서 다른 프레임을 방문하면 추가 context가 생성될 수 있으므로 브라우저 전체 context가 16개라는 뜻은 아니며 페이지 전체 renderer RAM 상한도 아니다. [WindowProxyManager](https://github.com/chromium/chromium/blob/156.0.8078.17/third_party/blink/renderer/bindings/core/v8/window_proxy_manager.h#L51). refs와 문서 연결은 다음 read에서 교체하고 탭 종료나 메인 문서 교체 때 해당 realm과 함께 해제된다. 새 renderer, 반복 타이머, worker와 파일 저장은 추가하지 않는다. 실제 RAM 전후 비교는 미실행이다.

## 검증 경계

모의 DOM은 문서 identity, 부모 연결, 행동 전후 검증과 예산을 검사한다. 실제 same-origin policy, 여러 realm의 native setter와 이벤트 전달, layout과 CSS transform, frame 탐색 및 renderer 경쟁을 재현하는 것은 아니다. `tests/shadow_smoke.py`에는 접근 가능한 프레임과 중첩 Shadow DOM, 폼 입력과 선택, Enter, 삭제 후 stale ref 및 opaque sandbox 제외를 확인할 fixture와 검사를 추가했다. 앱에 접속하는 이 검사는 아직 실행하지 않았다. 실제 앱이나 키체인 요청을 자동으로 시작하지 않는다. 최신 결과는 [작업 기록](work-log.md)을 기준으로 한다.

## 교차 출처 프레임 식별과 입력

2026-10-18 v8 소스 후보에는 `frames TAB` 메타데이터 조회, `read-frame TAB FRAME` 읽기와 최신 frame ref의 입력 연결이 있다. 현재 `supportedActions`는 `click`, `hover`, `fill`, `insert`, `press`, `check`, `select` 일곱 가지이며 `pointerActionsSupported:true`를 반환한다. 포인터 경로가 있다는 뜻이며 실제 요소의 노출이나 모든 transform을 보장하는 값은 아니다. 상세 호출과 반환은 [프레임 읽기와 입력](frame-read.md), session 전송과 정리는 [DevTools 연결 경로](devtools-routing.md), 프레임 포인터 단계는 [별도 계약](frame-pointer.md)을 따른다. [v8 통합 빌드](../artifacts/ego-parity-v8-build.json)는 성공했으며 앱을 실행하지 않았다.

후속 `read-page TAB`은 새 inventory의 root부터 너비 우선으로 최대 16개 문서를 시도하며, 성공 문서와 unread 사유·전체 frame 목록·합산 예산을 반환한다. 문서별 읽기는 자식 본문으로 재귀 진입하지 않으므로 같은 자식 본문을 여러 부모에 합치지 않는다. 순서는 각 session tree에서 관찰한 형제 순서이며 전체 DOM·화면 순서는 보장하지 않는다. [여러 프레임 읽기](page-read.md)를 포함한 [v9 통합 빌드](../artifacts/ego-parity-v9-build.json)는 성공했으며 실제 앱·CEF·E2E는 미실행이다.

`frames`는 현재 Browser의 root와 발견된 iframe child session을 지연 연결하고 각각 Page.enable → Target.setAutoAttach → Page.getFrameTree를 거친다. 응답은 예약 ID, 정확한 session과 실제 Browser를 확인한 뒤 사용한다. 탭 소유자·task revision과 메인 문서/navigation 세대를 유지 검사하며 사용자 공간·선택을 바꾸는 단계를 추가하지 않았다.

`frames` 항목은 `frameId`, `parentId`, `sessionId`, `url`, `name` 메타데이터다. DOM read의 `page.frames`는 한 read 안의 관찰 목록이므로 ID를 교환할 수 없다. URL이나 이름으로 DOM 권한 또는 입력 대상을 추정하지 않는다. metadata 응답은 `atomic:false`, `contentIncluded:false`이며 후자는 본문을 포함하지 않는다는 뜻이다.

child session은 Browser당 최대 16개·깊이 8에서 재사용한다. 전체 프레임 결과는 최대 128개, 한 FrameTree의 깊이는 32, 메타데이터 단계 응답은 256 KiB다. 같은 frame ID가 중복되면 덮어쓰지 않고 수집을 거절한다. 고정 Chromium의 각 session FrameTree는 로컬 프레임만 포함하고 OOPIF RemoteFrame 경계도 제외하므로 안정된 root·child tree는 분리된다. 여러 응답을 합친 tree는 원자적 전체 상태가 아니다.

`read-frame`은 새 inventory에서 지정 frame과 담당 session을 확인한 뒤 Runtime disable ACK → enable replay → 고정 `LightBrowserFrameControl` world 생성·재사용을 진행한다. numeric ID와 같은 session에서 보고된 frame·world·isolated context 이벤트를 대조하고, DOM 평가에는 uniqueContextId를 사용한다. `readFrameActions()`의 반환 ref에 native 요청별 접두어를 붙이며 focusedRef도 실제 반환 집합에 있는 것만 허용한다. 마지막 Runtime disable ACK 뒤 snapshot과 성공 응답을 공개한다.

Rust는 Browser당 최신 읽기 묶음 하나만 보관한다. read-frame은 문서 하나, read-page는 최대 16개 문서 binding과 합계 120개 ref를 게시한다. 새 read-frame/read-page를 inventory 준비 단계에서 수락하거나 관련 frame·제어권 수명이 바뀌면 이전 묶음을 무효화한다. 수락 뒤 실패해도 이전 ref를 복구하지 않지만 시작 전 PAGE_LOADING·TAB_BUSY 거절은 이전 묶음을 보존한다. 각 snapshot은 원래 Browser, 메인 세대, 소유권, frame/session, unique context와 documentId/generation에 묶인다. DOM은 WeakRef 요소와 WeakMap 문맥을 사용하고 같은 world의 다음 read에서 교체한다. 수거되거나 이동한 노드를 다른 요소로 재검색하지 않는다. 최신 Rust 묶음을 해제해도 다른 frame world의 약한 ref Map이나 고정 bridge가 즉시 없어지는 것은 아니다.

각 문서 읽기는 현재 frame과 열린 Shadow DOM만 포함한다. 자식 문서는 순회하지 않고 iframe 메타데이터를 `not-inspected`로 표시한다. read-frame의 기본 예산은 본문 12,000 UTF-16 units, 요소 120개, 노드 방문 10,000회, 열린 Shadow DOM 64개, 옵션 240개, 로컬 iframe 메타데이터 16개다. read-page는 이 여섯 예산을 문서 전체가 공유하고 남은 양만 다음 문서에 전달하며, 순회 깊이 64는 문서별로 적용한다. Read 응답은 파싱 전 2 MiB이고 다른 프레임 단계는 256 KiB다. read-page는 문서 JSON 합계와 metadata를 포함한 최종 응답에도 각각 2 MiB 상한을 적용한다.

frame action은 새 inventory에서 원래 frame/session을 다시 확인하고 실제 parentId 연결을 따라 부모부터 root까지 최대 32개를 검사한다. 각 부모의 정확한 session에서 고정 `LightBrowserFrameAncestors` world를 확정하고, getFrameOwner(childFrameId) → resolveNode(parent numeric context, objectGroup) → callFunctionOn(parent uniqueContextId, owner object 인수)를 실행한다. DOM은 연결된 HTML IFRAME/FRAME과 ownerDocument를 확인하고 composed ancestor의 숨김·0 크기·inert·aria-hidden·비활성을 검사한다. 다른 world에서 얻은 인수는 V8이 함수 실행 전에 거절한다. group release ACK와 부모 Runtime disable ACK를 받은 뒤 다음 부모로 진행한다.

비포인터 action은 모든 부모 표시 검사를 마친 뒤 대상 context의 unique ID가 snapshot과 같은지 확인하고 원래 documentId/generation으로 frameAct를 실행한다. DOM 준비·native 전송·사후 확인은 같은 session과 원래 ref를 사용하며 새 read나 context 대체로 이어가지 않는다. check·select와 일부 fill 유형은 기존 DOM 이벤트 계약이고, 지원 fill·insert·press는 기존 Chromium 입력 경로다. TAB_BUSY는 이 흐름과 마지막 Runtime disable ACK까지 유지한다. 명령 자체가 새 문서를 열어 원래 frame을 잃은 경우에도 frame 경로는 성공을 추정하지 않는다.

포인터는 원래 leaf에서 rect·viewport를 얻고 같은 parent owner 연결로 content mapping·viewport를 수집한다. Rust에서 부모 content와 viewport를 차례로 교차해 leaf 후보 영역을 만들고, leaf의 최대 9개 후보를 부모마다 실제 owner hit test로 거른다. 살아남은 후보의 leaf CSS point와 root CSS point는 따로 보관한다. root session의 mouseMoved ACK 후 leaf 동일점, 전체 부모 hit test와 원래 root point 일치, leaf 동일점 순서로 다시 확인한다. hover는 여기서 끝나며 click만 root 좌표의 mousePressed/mouseReleased를 보낸다. DOM 검사 session을 root 입력 session으로 바꾸거나 leaf 좌표를 그대로 root에 보내지 않는다.

준비 중 clip 또는 유효 후보가 없으면 leaf만 instant/center scrollIntoView를 한 번 요청하고 leaf·부모 geometry부터 모두 다시 수집한다. 후보 생성은 한 회 9개, 두 회 합계 18개다. 이는 부모 hit test와 이동 뒤 재검사를 모두 합친 호출 횟수가 아니다. 부모는 최대 32개이고 각 조회에서 owner objectGroup release와 Runtime disable ACK를 확인한다. mouseMoved를 보낸 뒤에는 스크롤이나 다른 후보로 자동 재시도하지 않는다.

지원되는 iframe mapping은 border/padding과 양수 축 정렬 CSS scale/zoom이며 실제 bounding rect와 계산한 크기를 대조한다. 회전·skew·perspective·motion path·SVG 조상, 확인할 수 없는 크기, visualViewport 확대·offset은 거절한다. devicePixelRatio를 직접 곱하지 않고 root CSS 좌표의 엔진 변환을 사용한다. 고정 Chromium의 leaf 스크롤은 일반 OOPIF 부모로 전달되지만 IPC와 compositor 반영은 비동기다. 큰 iframe의 특정 leaf 대신 owner 전체만 스크롤하는 것으로 성공을 추정하지 않는다. [포인터 소스 근거](../artifacts/frame-pointer-v1-sources.json)를 따른다.

단계 사이 pending이 없어도 active inventory가 현재 session·단계와 무효화 상태를 보관한다. 같은 session의 context 생성·파괴·초기화 이벤트를 대상과 모든 부모 tracker, 이미 수집한 문서들의 tracker에 전달해, 다른 부모를 검사하는 동안 앞서 확인한 world의 변경도 놓치지 않게 한다. CEF getter 재진입 중 frame/context/소유권이 바뀌면 다음 전송을 거절한다. 실패·취소·종료 때 snapshot과 로컬 registry를 비우고 root object group release, root Runtime disable, root auto-attach 해제를 독립 ACK로 관리한다. child의 객체와 Runtime은 session 폐기로 정리하며 모든 자손 detach 이벤트가 도착한다고 가정하지 않는다. 정리 중이거나 실패하면 새 discovery를 시작하지 않는다.

Runtime.disable은 reporting과 일부 Inspector 설정을 정리할 뿐, JS context·고정 world·remote object group을 없애거나 진행 중 evaluate를 취소하지 않는다. owner object는 명시적인 group release로 해제한다. 내부 content-only readFrame의 finally 해제와 현재 action용 약한 참조 보관을 혼동하지 않는다. 새 프로필·snapshot 저장소·renderer·반복 타이머는 추가하지 않았다. 실제 RAM은 미측정이다.

읽기 자체에는 부모 검사가 없으며, 비포인터 action의 부모 표시 검사도 검사 시점의 샘플이다. 키·텍스트 Input 메서드는 session을 지정해도 element나 unique context를 입력 대상으로 받지 않고 widget의 현재 포커스를 사용한다. 포인터에는 좌표·clipping·가림 검사를 추가했지만 비동기 스크롤·hit-test 정보·검사 뒤의 layout 변경을 원자적으로 막지는 않는다. 지연 focus handler나 동시 사람 입력으로 대상이 바뀌는 경합도 남는다. 사후 확인은 대상 불일치 보고이며 이미 전달한 효과를 취소하지 못한다. read-page의 합산 예산도 순차 관찰의 비원자성을 없애지는 않는다. [입력·부모 owner 소스 조사](../artifacts/frame-input-v1-sources.json)에 포커스 경계를 기록했다.

이전 content-only 구현의 [v6 빌드](../artifacts/ego-parity-v6-build.json)는 exit 0이었다. 다섯 입력 연결의 [v7 통합 빌드](../artifacts/ego-parity-v7-build.json)는 exit 0이며 소스 34개의 빌드 전후 해시가 일치한다. 포인터 연결의 [v8 통합 빌드](../artifacts/ego-parity-v8-build.json)는 exit 0, 91.49초이며 제품 소스·Cargo 35개의 전후 해시가 일치한다. 이후 read-page 변경의 [v9 통합 빌드](../artifacts/ego-parity-v9-build.json)는 exit 0, 95.05초이며 제품 소스·Cargo 36개의 전후 해시가 일치한다. 이번 단위·모의 DOM 전체 검사·앱·실제 CEF E2E는 실행하지 않았다. 소스 연결과 고정 엔진 조사를 Naru 실행 성공으로 대신하지 않는다. 교차 출처·OOPIF read→일곱 action, 부모 가림·부분 노출·스크롤·transform, 숨김/inert 거절, process swap·오래된 ref·취소·인계·cleanup, 배경 사용자 선택 보존은 [통합 E2E 08](ego-parity-e2e.md)에서 확인해야 한다. SOP 해제나 universal access는 사용하지 않으며, 이 구현만으로 교차 출처 제어 전체 완료를 선언하지 않는다.
