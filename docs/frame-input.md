# 프레임 참조로 입력하기

2026-10-18의 프레임 입력에 참조 스크롤을 더한 후속 소스 기준이다. `read-frame TAB FRAME`은 선택한 프레임, `read-page TAB`은 공통 예산으로 읽은 여러 프레임의 본문과 함께 `click`, `hover`, `fill`, `insert`, `press`, `check`, `select`, `scroll`에 사용할 요소 참조를 반환한다. 교차 출처 프레임도 현재 탭의 DevTools session과 isolated context를 확인해 연결한다. 페이지의 동일 출처 정책을 해제하거나 부모 문서에서 교차 출처 `contentDocument`를 읽는 방식은 아니다.

이 문서는 [프레임 읽기](frame-read.md), [여러 프레임 읽기](page-read.md)와 함께 원 문서에 결합한 입력 계약을 설명한다. 기존 `read TAB`과 같은 출처 iframe 제어는 그대로 유지한다. 현재 소스의 구현 범위이며 실제 Naru CEF 입력, 네이티브 화면, 외부 사이트 검증을 통과했다는 뜻은 아니다.

## CLI 사용

한 문서는 `frames TAB`의 `frameId`를 고른 뒤 `read-frame`으로 읽고, 여러 문서는 `read-page TAB`으로 함께 읽는다. 입력에는 전자의 `page.interactive[].ref` 또는 후자의 `documents[].page.interactive[].ref`를 그대로 사용한다. 작업 소유 탭은 같은 작업 ID와 현재 revision의 `task run` 안에서 읽기와 입력을 실행한다.

```sh
lightbrowser task run ID REV frames TAB
lightbrowser task run ID REV read-frame TAB FRAME
lightbrowser task run ID REV click TAB REF
lightbrowser task run ID REV hover TAB REF
lightbrowser task run ID REV fill TAB REF '전체 입력값'
lightbrowser task run ID REV insert TAB REF '선택 위치에 넣을 값'
lightbrowser task run ID REV press TAB REF Enter
lightbrowser task run ID REV check TAB REF true
lightbrowser task run ID REV select TAB REF '정확한 옵션 값'
lightbrowser task run ID REV scroll TAB REF 0 600
```

여러 문서를 읽을 때는 위의 frames/read-frame 대신 `lightbrowser task run ID REV read-page TAB`을 사용하고 같은 묶음 안의 각 frame ref로 액션을 실행한다. 새 읽기를 수락하면 이전 묶음이 교체되므로 두 읽기의 ref를 섞지 않는다.

위 명령은 사용 형태를 나타낸다. `ID`, `REV`, `TAB`, `FRAME`, `REF`를 실제 응답으로 바꾸며, 각 액션에 맞는 요소의 참조를 선택한다. 작업 소유가 없는 탭은 기존 권한 규칙에 따라 `task run ID REV` 없이 호출한다. frame ID는 요소 ref, 기존 `read`의 `page.frames[].id`, CEF Frame 식별자와 서로 교환하지 않는다.

`read-frame` 성공 응답의 `result`는 `tab`, `epoch`, `documentEpoch`, `frame`, `page`, `atomic:false`를 포함한다. `result`와 `page`에 `supportedActions:["click","hover","fill","insert","press","check","select","scroll"]`, `pointerActionsSupported:true`를 넣는다. `page.scope`는 `local-frame`이다. 반환된 `focusedRef`는 같은 결과의 `interactive`에 포함된 참조이거나 `null`이다.

공개 ref는 read-frame에서 `frame/<epoch>/<request-token>/<DOM-ref>`, read-page에서 문서 index가 추가된 `frame/<epoch>/<request-token>/<document-index>/<DOM-ref>` 형태의 내부 식별자다. 이를 직접 조합하거나 일부를 떼어내지 않는다. 요청 token과 문서 index는 별도 CLI 인자가 아니다. Rust는 해당 탭의 최신 읽기 묶음에 실제로 등록된 ref만 받아들인다. read-page의 각 page도 `scope:local-frame`과 해당 문서의 focusedRef를 유지하며 합산 반환은 [별도 계약](page-read.md)을 따른다.

## 지원 동작

| 명령 | 프레임 안에서의 동작 |
| --- | --- |
| `fill` | text, search, tel, url, password, textarea와 지원하는 contenteditable을 전체 선택해 Chromium 텍스트 입력으로 교체하고 결과를 확인한다. email, number 등 기존 나머지 유형은 DOM 경로를 유지한다. |
| `insert` | 지원 입력칸과 contenteditable의 기존 선택을 유지해 Chromium 텍스트 입력을 요청한다. 입력칸은 정확한 최종 값, contenteditable은 입력 이벤트 관찰로 검증한다. |
| `press` | 기존 42개 지원 키의 포커스를 준비하고 같은 frame session으로 키 입력을 보낸다. 키다운 대상 관찰과 엔진 ACK의 의미를 구분한다. |
| `check` | 기존 HTML checkbox, radio 정책에 따라 DOM 클릭과 상태 확인을 사용한다. 물리 입력이나 trusted click 보장은 아니다. |
| `select` | 기존 HTML 단일 select 정책에 따라 정확한 옵션 값으로 설정한다. 합성 이벤트 경로와 옵션 상한을 유지한다. |
| `scroll TAB REF DX DY` | `scroll` 메타데이터가 있는 목록이나 문서에 DOM `scrollBy`를 호출하고 직후 좌표를 반환한다. 포커스를 주거나 조상으로 이동을 넘기지 않는다. |
| `click`, `hover` | 로컬 후보와 모든 부모의 좌표 변환, hit test를 확인해 root session으로 Chromium 포인터 입력을 보낸다. hover 뒤에는 처음 선택한 local 좌표와 root 좌표를 다시 검사한다. |

포인터의 좌표, 부모 가림, 전송 단계와 제한은 [프레임 포인터 입력](frame-pointer.md)을 따른다. 나머지 입력 종류, 성공 필드와 실패 의미는 [fill](native-fill-control.md), [insert](insert-text.md), [키 입력](keyboard-input.md), [체크 상태](check-control.md), [단일 선택](select-control.md)의 기존 계약을 따른다. 각 문서의 과거 검증은 새 프레임 라우팅을 검증한 결과가 아니다. `scroll TAB DX DY`는 기존 페이지 스크롤 명령으로 유지한다. 새 `scroll TAB REF DX DY`는 프레임의 목록이나 문서 참조를 사용하며 [참조 스크롤 계약](targeted-scroll.md)을 따른다. 동기적인 이동 관찰은 비동기 목록 로딩이나 네이티브 휠 입력의 증거가 아니다.

## 실행 대상과 부모 검사

`src/browser.rs`의 `make_frame_snapshot`은 Browser, 탐색 epoch, 문서 epoch, 작업 소유자와 상태, frame ID, session ID, `uniqueContextId`, DOM의 `documentId`, 읽기 `generation`, 허용 ref 목록을 묶는다. 입력 시 `start_frame_action`은 최신 읽기 묶음에서 ref가 등록된 snapshot을 고르고 현재 frame tree를 다시 수집한다. 다른 frame이나 새 context로 대상을 바꾸지 않는다.

입력 전에는 최대 32단계의 부모 연결을 확인한다. `fill`, `insert`, `press`, `check`, `select`, `scroll`의 부모 상태 검사는 다음과 같다. 각 부모의 isolated context에서 `DOM.getFrameOwner`, `DOM.resolveNode`, `Runtime.callFunctionOn`을 사용해 실제 iframe 요소를 검사한다. `inspectFrameOwner`는 연결된 HTML IFRAME 또는 FRAME인지, 해당 부모 문서에 속하는지, 열린 Shadow DOM과 slot을 포함한 조상에서 숨김, inert, disabled 상태인지 확인한다. `contentDocument`, 포커스와 스크롤은 건드리지 않는다. 임시 원격 객체는 object group 해제 ACK를 거쳐 정리한다.

이 여섯 액션의 검사는 부모가 레이아웃 박스를 갖고 숨겨지거나 비활성화되지 않았는지 확인하는 범위다. 다른 요소가 덮은 상태, clipping, 최상위 viewport 안에 있는지와 포인터 좌표는 확인하지 않는다. 각 부모 확인과 실제 입력 사이에 페이지가 바뀔 수도 있다. `click`, `hover`는 별도로 `inspectFramePointer`를 호출해 부모 content box, viewport, 배율과 정확한 owner hit test까지 확인한다.

대상 context를 다시 확인한 뒤 `frameAct(request, {documentId,generation})`를 호출한다. DOM bridge는 바인딩을 먼저 확인하므로 오래된 요청이 현재 입력 관찰자를 지우지 않는다. 약한 참조에서 현재 노드를 찾고 원문서, 읽기 세대, 연결과 편집 조건을 다시 검사한다. 일반 `act`로 프레임 바인딩을 우회할 수 없다.

DOM 준비와 사후 검증은 원래 frame session, `uniqueContextId`, ref와 DOM 바인딩을 사용한다. 키와 텍스트 입력은 대상 frame session으로 보내고, 포인터 입력은 부모 변환을 마친 최상위 좌표를 사용해 root session으로 보낸다. 준비부터 입력, 결과 확인과 Runtime disable ACK까지 같은 요청 수명을 유지한다. 같은 탭의 page 작업은 `TAB_BUSY` 경계를 사용한다. 사용자 공간과 선택 탭을 바꾸거나 창을 앞으로 가져오는 단계를 추가하지 않는다.

## 참조 수명과 실패

Rust는 탭마다 최신 프레임 읽기 묶음 하나를 유지한다. read-frame은 문서 하나, read-page는 최대 16개 문서 binding과 합계 120개 ref를 보관한다. 새 read-frame/read-page가 inventory 준비 단계에 수락되면 이전 묶음을 무효화하므로 그 뒤 새 읽기가 실패해도 이전 ref를 다시 사용하지 않는다. 반면 시작 전 PAGE_LOADING·TAB_BUSY 거절은 이전 묶음을 보존한다. frame 연결 변경, 문서나 context 교체, 작업 제어권 변경, 취소, renderer 종료와 탭 종료도 현재성 검사에 포함한다. 같은 URL이나 같은 frame ID만으로 이전 문서를 유지한다고 판단하지 않는다.

오래된 참조는 `STALE_FRAME_SNAPSHOT`, 연결이나 문서 변경은 `FRAME_CHANGED`, 부모 검사 실패는 `FRAME_OWNER_UNAVAILABLE` 등으로 실패한다. 읽을 문서가 준비되지 않았으면 `PAGE_LOADING`, 현재 목록에서 대상이 없으면 `FRAME_NOT_FOUND`이며 상한 때문에 목록을 완성하지 못한 경우에는 `FRAME_LIMIT`가 될 수 있다. 실패 후 자동으로 새 문서를 읽어 같은 입력을 반복하지 않는다.

키·Chromium 텍스트 입력은 [임시 포커스 준비](focus-input.md)를 해당 session에 적용한다. enable ACK 뒤 새 준비를 수행하고 입력 후 disable ACK, 읽기 전용 결과 확인, 기존 Runtime 정리 순서로 끝낸다. root ACK가 모든 OOPIF를 준비했다는 뜻은 아니다. 준비와 실제 입력은 원자적이지 않다. 지연된 focus 핸들러, 준비 후 DOM 변경, 입력 뒤 이동과 페이지 자체 처리로 결과가 달라질 수 있다. 엔진에 접수한 입력을 취소나 오류로 되돌릴 수 없다. `attempted`, `outcomeUnknown`과 액션별 관찰 결과를 확인하고 현재 문서를 다시 읽어 판단한다. 엔진 ACK를 제출이나 서버 저장 성공으로 해석하지 않는다.

## 메모리와 읽기 한계

`readFrameActions()`는 현재 문서와 열린 Shadow DOM만 읽고 자식 iframe 본문은 내려가지 않는다. read-frame의 기본 예산은 본문 12,000 UTF-16 units, 요소 120개, 노드 방문 10,000회, 열린 Shadow DOM 64개, select 옵션 240개, `not-inspected` iframe 메타데이터 16개다. read-page는 이 여섯 예산을 문서 전체가 공유하고 남은 양만 다음 문서에 전달한다. 순회 깊이 64는 문서별로 적용한다. read-page의 최대 16번 시도와 문서 JSON 합계·최종 응답 각각 2 MiB 제한은 [공통 예산 계약](page-read.md)을 따른다.

프레임 refs Map은 노드와 context를 강하게 붙잡는 대신 `WeakRef(element)`와 generation만 저장한다. 액션 동안 현재 노드의 ownerDocument에서 WeakMap context를 찾아 임시로 사용한다. WeakRef를 사용할 수 없으면 강한 참조로 대체하지 않고 읽기를 거절한다. 다만 같은 문서에 노드가 계속 연결되어 있으면 문서 자체가 노드를 유지하므로 약한 참조가 즉시 사라지는 것은 아니다.

기존 내부 `readFrame()`은 본문 전용 경로로 남아 반환과 예외에서 refs와 contexts를 해제한다. 현재 native `read-frame`과 `read-page`는 이 함수 대신 `readFrameActions()`를 사용한다. DOM의 `clearFrame(binding)`은 정확히 같은 세대를 명시적으로 해제하고 반복 호출을 허용하지만, 현재 Rust 경로는 이 API를 호출하지 않는다. 따라서 새 snapshot으로 바꾼 뒤 이전 world의 모든 메타데이터가 즉시 지워졌다고 설명하지 않는다.

각 world의 약한 ref 목록과 context 메타데이터는 그 world의 새 읽기로 교체되거나 엔진 문서 수명까지 남을 수 있다. 고정 world를 재사용하며 `Runtime.disable`은 world나 JS 메모리를 파괴하는 명령이 아니다. press와 insert의 관찰자는 결과 소비, 새 동작, 새 읽기 또는 5초 단발 만료로 정리한다. renderer가 멈추면 타이머 실행도 늦어질 수 있다. Rust의 최신 묶음 하나라는 상한을 탭 전체 isolated world의 메모리 상한으로 확대하지 않는다.

새 저장소, UI renderer, 상주 worker와 반복 타이머는 추가하지 않는다. 읽기 값의 password 제외와 페이지 데이터의 비신뢰성도 기존 계약을 유지한다. 실제 RAM 변화는 측정하지 않았다.

## 검증 상태

이번 문서 작성에서 protocol과 CLI, `start_frame_action`, 부모 검사, `make_frame_snapshot`, 입력 전송과 검증, DOM bridge의 바인딩 및 해제 경로를 소스로 확인했다. 후속 포인터 연결은 `frame_pointer.rs`의 clipping과 후보 검증, 부모별 context 추적, root 입력과 child DOM 응답 구분까지 읽기 리뷰했다. DOM 변경에는 `node --check` 구문 검사만 실행했다. 새 프레임 입력 경로의 단위 검사, 모의 DOM 전체 검사, Naru CEF, 네이티브 UI, 외부 사이트와 통합 E2E는 실행하지 않았다. 후속 [v8 통합 빌드](../artifacts/ego-parity-v8-build.json)는 exit 0, 91.49초였으며 제품 소스와 Cargo 파일 35개의 전후 해시가 같았다. 빌드는 root가 실행했고, 이 문서 갱신에서는 해당 기록을 확인했다. 이후 read-page의 공통 예산과 여러 문서 ref 묶음은 별도 [v9 통합 빌드](../artifacts/ego-parity-v9-build.json)에서 exit 0, 95.05초와 제품 소스·Cargo 36개의 전후 해시 일치를 확인했다. 실제 앱·CEF·E2E와 전체 회귀 검사는 실행하지 않았으며 과거 v8 결과와 구분한다. 자세한 결과는 [작업 기록](work-log.md)을 따른다.

실제 검증에는 같은 출처와 교차 출처 프레임, OOPIF, 중첩 부모와 열린 Shadow DOM, 숨김과 비활성 부모, 입력 뒤 탐색, 오래된 ref, 새 읽기의 실패, 작업 인계와 취소, 배경 입력 중 사용자 선택 유지가 필요하다. 현재 포인터 연결은 통합 빌드까지 완료했으며 실제 좌표 변환, 가림 거절과 root 입력은 통합 E2E에서 확인해야 한다. 이전 빌드 성공이나 기존 root 포인터 검증을 이 새 경로의 성공 근거로 사용하지 않는다.
