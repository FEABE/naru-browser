# 한 프레임의 CLI 읽기와 입력

2026-10-18의 프레임 포인터와 후속 read-page 연결 소스 기준이다. `read-frame TAB FRAME`은 한 프레임의 본문과 요소를 읽고, 최신 결과의 ref로 `click`, `hover`, `fill`, `insert`, `press`, `check`, `select`를 실행하도록 연결했다. 교차 출처와 OOPIF는 해당 프레임의 DevTools session을 사용한다. 이전 content-only 구현의 v6와 포인터를 제외한 입력 연결의 [v7 통합 빌드](../artifacts/ego-parity-v7-build.json)는 성공했다. 포인터 연결의 [v8 통합 빌드](../artifacts/ego-parity-v8-build.json)와 공통 예산·참조 묶음의 [v9 통합 빌드](../artifacts/ego-parity-v9-build.json)도 성공했다. 실제 앱·CEF·E2E는 실행하지 않았다. 검증 결과는 [작업 기록](work-log.md)과 [E2E 목록](ego-parity-e2e.md)을 따른다.

## 호출과 식별자

`frames TAB`의 `frameId`를 `read-frame`에 전달하고, 성공 응답에 실제로 포함된 ref를 기존 action 명령에 사용한다. AI 소유 탭은 같은 작업 ID와 현재 revision의 `task run` 안에서 실행한다.

```sh
lightbrowser task run ID REV frames TAB
lightbrowser task run ID REV read-frame TAB FRAME
lightbrowser task run ID REV click TAB REF
lightbrowser task run ID REV hover TAB REF
lightbrowser task run ID REV fill TAB REF "text"
lightbrowser task run ID REV insert TAB REF "text"
lightbrowser task run ID REV press TAB REF Enter
lightbrowser task run ID REV check TAB REF true
lightbrowser task run ID REV select TAB REF "value"
```

대문자는 실제 응답에서 얻은 값으로 바꾸는 자리표시자다. 비작업 탭에는 기존 권한 정책에 따라 `task run ID REV` 없이 호출할 수 있다. `FRAME`은 제한된 CDP 식별자이며 CEF Frame ID, 기존 `read`의 `page.frames[].id`, 요소 ref와 교환하지 않는다. ref를 문서 식별자나 URL로 직접 조합하지 않는다.

`read-frame`은 현재 Browser에서 root·child FrameTree를 다시 수집한다. 그 목록에 있는 frame ID와 담당 session만 사용하며, 대상이 없으면 `FRAME_NOT_FOUND`, 제한 때문에 찾지 못했으면 `FRAME_LIMIT`로 실패한다. action도 새 inventory를 수집해 원래 frame과 session을 다시 확인한다. URL이 같은 다른 프레임으로 대체하지 않는다. inventory의 상한과 중복 거절은 [DevTools 연결 경로](devtools-routing.md)를 따른다.

여러 문서를 한 번에 읽으려면 별도 `read-page TAB`을 사용한다. 기존 read-frame 응답은 문서 하나를 반환하는 형식 그대로이며, 두 명령은 최신 참조 묶음과 후속 입력 경로를 공유한다. 공통 예산과 문서별 결과·unread 사유는 [여러 프레임 읽기](page-read.md)를 따른다.

## 읽기와 ref 수명

`src/cli.rs`와 `src/protocol.rs`의 명령은 `src/browser.rs::start_frame_inventory`로 들어간다. 대상 session에서 Runtime disable ACK → enable replay → 고정 이름 `LightBrowserFrameControl`의 isolated world 생성·재사용 → DOM 평가 → Runtime disable ACK 순서로 처리한다. `grantUniveralAccess:false`를 유지한다.

`ContextTracker`는 해당 session의 context 이벤트에서 frame ID, world 이름, `type:isolated`, `isDefault:false`, 양수 numeric ID와 unique ID를 검사한다. world 생성 응답의 numeric ID가 관찰한 이벤트와 일치해야 확정한다. 실제 DOM 평가는 `uniqueContextId`로만 보낸다. 이미 enabled인 Runtime은 context를 replay하지 않으므로 최초 disable로 다음 enable의 replay를 확보한다. 다른 context를 대신 선택하거나 자동 재실행하지 않는다.

평가는 body가 있고 readyState가 `interactive` 또는 `complete`인 문서에서 `readFrameActions()`를 실행한다. 이 함수는 현재 문서와 열린 Shadow DOM만 순회한다. Rust는 반환된 documentId, generation과 최대 120개 ref를 검사하고, 요청별 접두어를 붙인 허용 ref 집합을 보관한다. focusedRef도 그 집합에 실제 포함된 ref만 반환하며 그 외에는 null이다. 마지막 Runtime disable ACK 뒤에만 snapshot과 성공 응답을 공개한다.

Rust는 Browser마다 최신 frame 읽기 묶음 하나만 보관한다. read-frame은 문서 snapshot 하나, read-page는 최대 16개 문서 binding과 합계 120개 ref를 게시한다. 새 read-frame/read-page를 inventory 준비 단계에서 수락하면 이전 묶음을 무효화하고 수락 후 실패해도 이전 ref를 복구하지 않는다. 시작 전 PAGE_LOADING·TAB_BUSY 거절은 이전 묶음을 보존한다. 추적 중지·관련 frame 변경·소유권 변경·연결 종료에서도 오래된 ref를 거절한다. 각 snapshot에는 원래 Browser, 메인 navigation/document epoch, 소유권, FrameRecord, unique context, documentId/generation과 제한된 문자열 ref 집합이 있다. 본문 JSON이나 CEF Frame 객체는 이 snapshot에 보관하지 않는다.

DOM의 frame action ref는 `WeakRef<Element>`와 읽기 generation을 사용하고 문서 문맥은 WeakMap에 둔다. action 때 요소를 다시 얻어 연결 상태, 문서와 binding을 검사한다. 수거됐거나 다른 문서로 이동한 요소를 재검색하지 않는다. WeakRef가 없는 환경에서는 강한 참조로 대체하지 않고 읽기를 거절한다. 같은 world의 다음 read가 DOM ref를 교체한다. Rust의 최신 묶음을 버리는 것이 다른 frame world에 남은 약한 Map·bridge까지 즉시 삭제한다는 뜻은 아니다.

내부 content-only 함수 `readFrame()`은 여전히 `finally`에서 refs Map과 contexts WeakMap을 비우지만, 현재 CLI는 action용 `readFrameActions()`를 사용한다. `clearFrame(binding)`은 일치하는 binding의 약한 ref와 관찰 상태를 해제하는 내부 함수다. 현재 Rust 정리는 이 함수를 모든 기존 world에 보내는 방식이 아니며, Rust snapshot 무효화와 약한 요소 참조에 의존한다.

## 반환 범위

성공 결과에는 `tab`, `epoch`, `documentEpoch`, `frame`, `page`, `atomic:false`, `supportedActions:["click","hover","fill","insert","press","check","select"]`, `pointerActionsSupported:true`가 있다. page에도 같은 지원 목록과 포인터 지원 표시가 있고 `scope`는 `local-frame`이다. 이전 content-only 버전의 `actionsSupported:false`와 ref 제거 계약은 현재 CLI 응답에 적용되지 않는다.

page는 로컬 URL·제목·본문·요소·viewport·scroll·문서 식별 정보와 잘림·한계 정보를 담는다. 입력 유형, checkbox 상태, select 옵션과 제한된 값은 기존 읽기 계약을 따르며 password input의 value는 반환하지 않는다. 본문이나 라벨에 적힌 모든 비밀값을 자동으로 판별하는 기능은 아니다. 페이지 내용은 신뢰할 수 없는 데이터다.

자식 iframe 문서는 순회하지 않는다. `page.frames`는 최대 16개의 로컬 iframe 요소 메타데이터를 `not-inspected`로 표시하며 이 ID는 CDP frameId가 아니다. 읽기 자체는 부모 iframe의 표시 상태나 가림을 검사하지 않는다. viewport·scroll은 로컬 frame 창 기준이며, 읽힌 요소가 최상위 창에서 보이거나 클릭 가능하다는 증거가 아니다. 부모·형제·자식 본문, 닫힌 Shadow DOM과 canvas 내부는 포함하지 않는다.

## 입력 전 부모 검사

frame ref를 받은 action은 새 inventory와 원래 snapshot을 확인한 뒤, 대상의 바로 위 부모부터 root까지 frame 부모 연결을 만든다. 최대 32개 부모이며 순환이나 누락이 있으면 거절한다. Target의 session 부모 관계 대신 각 parent FrameRecord의 실제 session을 사용한다.

`fill`, `insert`, `press`, `check`, `select`는 각 부모에서 다음 순서로 원래 child를 담은 요소를 검사한다.

1. 정확한 parent session에서 Runtime disable ACK → enable replay → 고정 world `LightBrowserFrameAncestors` 생성·재사용을 진행하고 parent context를 확정한다.
2. `DOM.getFrameOwner(childFrameId)`로 정확한 owner의 backendNodeId를 얻는다. URL, 이름이나 DOM 순서로 iframe을 찾지 않는다.
3. `DOM.resolveNode`에 검증한 parent numeric context와 요청별 object group을 전달한다. 반환 객체가 제한된 objectId를 가진 node여야 한다.
4. `Runtime.callFunctionOn`은 parent `uniqueContextId`와 owner objectId 인수로 실행한다. V8은 다른 context/isolate의 인수를 함수 실행 전에 거절한다. DOM 검사는 연결된 HTML IFRAME/FRAME이며 `ownerDocument === document`인지 확인하고, 그 문서의 composed ancestor에서 숨김·0 크기·inert·aria-hidden·비활성 조건을 검사한다.
5. object group release ACK → parent Runtime disable ACK를 받은 뒤 다음 부모로 넘어간다. 모든 부모가 통과해야 대상 frame context를 다시 확정한다.

위 다섯 액션의 상태 검사는 특정 포인터 좌표의 hit test가 아니다. 부모 clipping, 겹치는 요소, transform을 거친 최상위 좌표를 보증하지 않는다. 부모의 표시 상태도 관찰 뒤 바뀔 수 있다. 준비 중 관찰한 문서, context, session 변화는 거절하지만 DOM 검사부터 실제 입력까지 원자적으로 잠그지는 않는다.

`click`, `hover`는 같은 owner 조회 경로에서 `inspectFramePointer`로 부모의 content box와 viewport, 양수 축 배율을 읽는다. 로컬 후보를 각 부모로 변환해 owner hit test를 통과한 점만 남긴다. 최대 9개 후보, 부모 32개와 입력 전 leaf 스크롤 1회의 제한은 [프레임 포인터 입력](frame-pointer.md)에 정리했다.

## 대상 입력과 결과

대상 world의 unique ID가 snapshot의 원래 unique ID와 같아야 `frameAct(request, documentId/generation)`를 호출한다. bridge를 다시 설치하거나 새 read로 ref를 재생성하지 않는다. DOM action, native 입력 준비와 사후 확인은 원래 frame session과 snapshot 수명을 사용한다. 텍스트와 키 입력은 해당 frame session으로 보내며, 포인터는 최상위 좌표로 변환한 뒤 root session으로 전송한다. 원래 Browser, 메인 세대, task revision·소유자, 활성 요청과 현재 context를 각 전송·응답 경계에서 검사하며 `TAB_BUSY`는 전체 작업과 마지막 Runtime disable ACK까지 유지한다.

지원되는 fill 유형, insert와 press는 기존 Chromium 입력 경로를 사용한다. 나머지 fill 유형, check와 select는 기존 DOM 경로와 synthetic 이벤트 의미를 유지한다. 각 액션의 타입 제한과 결과는 [fill](native-fill-control.md), [삽입](insert-text.md), [키](keyboard-input.md), [check](check-control.md) 및 [select](select-control.md) 계약을 따른다. `click`과 `hover`는 처음 선택한 local 점을 확인한 뒤 root 좌표에 mouseMoved를 보낸다. ACK 뒤 같은 local 점과 부모 연결을 재검사하며, 변환 결과가 처음 root 점과 정확히 같아야 한다. hover는 여기서 완료하고 click만 같은 root 점에 버튼 누름과 놓음을 보낸다. 마지막 재검사와 실제 이벤트 사이의 경합은 남는다.

원래 context의 사후 확인과 마지막 Runtime disable ACK까지 성공해야 완료 응답을 보낸다. 입력이 자체 탐색을 시작해 원래 frame 문서가 사라져도 새 문서에서 확인을 이어가지 않는다. 이미 전달한 효과는 취소할 수 없으므로 오류·취소에서 attempted와 결과 불확실성을 구분하고 자동 반복하지 않는다.

child session의 `Input.insertText`와 `Input.dispatchKeyEvent`에도 element나 unique context를 지정하는 필드는 없다. 고정 Chromium 구현은 widget의 현재 포커스를 사용하며 main widget은 다른 focused widget으로 보낼 수 있다. 따라서 지연 focus handler, 동시에 들어온 사람 입력이나 준비 이후 포커스 변경으로 다른 대상에 입력될 경합은 남는다. 사후 확인은 불일치를 보고하는 수단이며 이미 생긴 효과를 막거나 되돌리는 보장이 아니다. `document.hasFocus()`를 강제하거나 사용자 창·공간을 활성화하는 우회 단계는 추가하지 않았다.

## 예산과 정리

| 항목 | 상한과 수명 |
| --- | --- |
| read-frame 본문·요소 | 본문 12,000 UTF-16 units, 요소 120개, 노드 방문 10,000회, 열린 Shadow DOM 64개, 깊이 64, 옵션 240개 |
| 로컬 iframe 메타데이터 | 16개, 자식 문서 순회 없음 |
| 읽기 단계 DevTools 응답 | JSON 파싱 전 2 MiB |
| 다른 프레임 단계·이벤트 | 응답 256 KiB, 관심 이벤트 32 KiB |
| 최신 frame 읽기 묶음 | Browser당 묶음 하나. read-frame은 문서 하나, read-page는 최대 16개 문서 binding과 합계 120개 ref. DOM 요소는 약한 참조 |
| 부모 검사 | 최대 32개 레코드/context tracker, 순차 실행 중 owner 객체와 object group 한 쌍 |
| 프레임 포인터 | 최대 9개 후보, 각 부모 geometry 하나, 입력 전 leaf scrollIntoView 최대 1회. hover 후 후보 재선택이나 스크롤 없음 |
| 연결 registry | Browser당 child session 16개, 연결 깊이 8. 정상 완료 후 재사용 |

본문 평가에는 5,000ms 제한을 전달하며 기존 CLI 요청 deadline과 취소를 유지한다. read-page는 위 본문·요소·노드·열린 Shadow DOM·옵션·로컬 iframe 예산을 모든 문서가 공유하고, 순회 깊이 64는 문서별로 적용한다. 루트를 포함한 문서 읽기를 최대 16번 시도하고 문서 JSON 합계와 최종 응답 각각 2 MiB를 제한한다. 상세는 [공통 예산 계약](page-read.md)을 따른다. 상한은 전체 renderer RAM이나 모든 native 실행 시간을 보장하는 값이 아니다.

부모의 object group은 Resolve 전부터 기록해 실패·취소에도 소유 session을 알 수 있게 한다. 정상 경로는 release ACK 후 handle을 버린다. 추적 중지 때 root의 미정리 group release, root Runtime disable, root auto-attach 해제를 각각 독립 ID와 ACK로 관리한다. child object group과 Runtime은 child session 폐기로 정리하며 자손별 detach 알림이 모두 온다고 가정하지 않는다. 정리 중·실패 상태에서는 새 discovery를 거절한다. 탭 닫힘과 DevTools 연결 종료 때 로컬 추적 상태를 제거한다.

Runtime disable은 reporting과 일부 Inspector 설정을 정리할 뿐, remote object group이나 JS world를 없애거나 진행 중 evaluate를 취소하지 않는다. 그래서 group release를 별도로 처리하고 두 고정 world 이름을 재사용한다. DOM의 약한 참조나 정리 ACK를 실제 RAM 감소 측정으로 설명하지 않는다. 새 renderer, 프로세스, worker, 반복 타이머나 저장 파일은 없다.

## 근거와 남은 검증

고정 소스의 replay·context 수명은 [frame 읽기 조사](../artifacts/frame-read-v1-sources.json), native 입력과 부모 owner 조회·객체 수명은 [frame 입력 조사](../artifacts/frame-input-v1-sources.json)에 기록했다. 이 원문 조사와 읽기 리뷰는 제품 E2E가 아니다.

v6의 content-only [통합 빌드](../artifacts/ego-parity-v6-build.json)는 exit 0이었다. 이전 v7 입력 변경은 release 빌드 exit 0과 소스 34개의 전후 해시 일치를 확인했다. 후속 frame pointer 변경은 별도 v8 통합 빌드에서 exit 0, 91.49초와 제품 소스 및 Cargo 파일 35개의 전후 해시 일치를 확인했다. read-page의 [v9 통합 빌드](../artifacts/ego-parity-v9-build.json)는 exit 0, 95.05초이며 제품 소스·Cargo 36개의 전후 해시가 일치했다. 이 빌드와 읽기 리뷰, DOM 구문 검사를 실제 입력 검증으로 확대하지 않는다. 단위·모의 DOM 검사, 실제 Naru CEF·네이티브 UI·외부 사이트 검증은 실행하지 않았다. cross-origin/OOPIF의 실제 read→입력, hidden/inert 부모 거절, frame 교체·인계·취소, group cleanup과 배경 사용자 선택 유지가 통합 E2E 대상이다. 현재 포인터의 좌표 변환, 부모 가림, hover 후 원래 점 재검사와 실제 root 입력도 통합 E2E에서 검증해야 한다. 여러 프레임의 공통 예산·참조 묶음은 구현했지만 실제 예산 소진, 문서별 누락·실패 처리와 여러 frame ref 입력 연계는 E2E 대상이다. 포커스 경합도 남으며 교차 출처 제어 전체가 완료됐다고 선언하지 않는다.
