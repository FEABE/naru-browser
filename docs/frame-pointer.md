# 한 프레임의 참조로 클릭과 hover 실행하기

2026-10-18의 프레임 포인터 연결 소스 기준이다. `read-frame TAB FRAME`이 반환한 ref에 `click`과 `hover`를 연결했다. leaf 문서의 좌표와 hit test는 해당 문서에서 확인하고, 부모 iframe의 좌표 변환과 가림은 각 부모의 isolated context에서 따로 확인한다. 최종 포인터 입력은 최상위 viewport 좌표로 root DevTools session에 보낸다.

[v8 통합 빌드](../artifacts/ego-parity-v8-build.json)는 `./scripts/dev.sh --release --build-only`로 91.49초에 exit 0을 기록했다. 제품 소스와 Cargo 파일 35개의 전후 해시가 같았다. DOM 구문 검사와 읽기 리뷰도 수행했다. 단위 검사, 모의 DOM 검사, 실제 Naru CEF, 네이티브 UI와 외부 사이트 E2E는 실행하지 않았다. 빌드 성공과 기존 root 포인터 검증을 이 새 연결의 실제 입력 증거로 사용하지 않는다.

## CLI와 읽기 범위

```sh
lightbrowser task run ID REV frames TAB
lightbrowser task run ID REV read-frame TAB FRAME
lightbrowser task run ID REV hover TAB REF
lightbrowser task run ID REV click TAB REF
```

각 대문자는 실제 응답의 값으로 바꾼다. ref를 직접 조합하거나 URL이 같은 다른 프레임의 ref로 대체하지 않는다. 작업 소유가 없는 탭은 기존 권한 규칙에 따라 `task run ID REV` 없이 호출한다.

`read-frame`의 `supportedActions`는 `click`, `hover`, `fill`, `insert`, `press`, `check`, `select`이며 `pointerActionsSupported:true`다. 이 표시는 연결된 API의 범위다. 읽기에 포함된 모든 요소가 클릭 가능하거나 모든 CSS geometry를 지원한다는 뜻은 아니다.

`page.scope:local-frame`과 viewport는 선택한 프레임 기준이다. 읽기 자체는 부모 가림을 검사하지 않는다. 입력 요청에서 새 frame inventory와 원래 snapshot을 확인한 뒤 부모 연결을 검사한다. 기존 `read TAB`과 같은 출처 iframe 포인터 경로는 바꾸지 않는다. 참조, 문서와 제어권 수명은 [프레임 입력](frame-input.md)을 따른다.

## 준비와 좌표 변환

`src/browser.rs`의 `start_frame_action`은 해당 ref가 속한 snapshot으로 `FramePointer`를 만든다. snapshot에는 Browser, 탐색과 문서 epoch, 작업 소유권, frame과 session, unique context, DOM documentId와 generation이 묶여 있다. 새 inventory에서도 같은 frame과 session이어야 한다.

1. leaf의 원래 unique context에서 `frameAct`의 geometry 단계를 실행한다. ref의 현재성, 표시와 비활성 상태를 검사하고 요소의 bounding rect와 로컬 viewport를 반환한다. 첫 시도에서는 스크롤하지 않는다.
2. leaf의 바로 위 부모부터 최상위 문서까지 최대 32개 연결을 만든다. 각 부모의 검증된 context에서 `DOM.getFrameOwner`, `DOM.resolveNode`, `Runtime.callFunctionOn`으로 정확한 iframe owner를 전달한다. URL이나 DOM 순서로 owner를 찾지 않는다.
3. `inspectFramePointer`는 연결된 HTML IFRAME 또는 FRAME인지, 해당 부모 문서에 속하는지 확인한다. composed ancestor의 숨김, inert와 비활성 상태도 검사한다. child의 `contentDocument`에 접근하지 않는다.
4. 부모의 computed border, padding, content box와 CSS 배율로 로컬 점을 부모 viewport 점에 매핑한다. 각 축은 `parentX = content.left + localX * scaleX`, `parentY = content.top + localY * scaleY`다. 배율은 유한한 양수여야 한다.
5. `src/frame_pointer.rs::local_clip`이 leaf rect와 viewport를 교차하고 각 부모 content box와 viewport로 차례대로 자른다. 이 결과를 역변환해 leaf에서 후보를 고를 영역을 구한다.
6. leaf candidates 단계는 그 영역에서 고정된 최대 9개 점을 hit test한다. 열린 Shadow DOM 안쪽까지 확인하며, 원래 요소 안의 별도 버튼이나 링크 같은 interactive descendant를 대신 누르는 점은 제외한다.
7. 후보를 부모마다 변환한다. 변환한 점이 content box와 viewport 안에 있고 부모의 deep hit 결과가 정확한 owner일 때만 남긴다. index를 유지해 최상위까지 살아남은 첫 후보와 원래 leaf 점을 짝짓는다.

Rust는 JSON의 좌표, index와 개수를 다시 검사한다. 좌표와 rect는 유한한 값이며 절댓값 1,000,000 이내다. 포인터 후보는 음수 좌표를 허용하지 않는다. index는 0부터 8까지이며 중복이나 없던 index를 추가한 결과를 거절한다. 부모 변환 결과와 후보 좌표의 차이는 0.01 CSS px 이하여야 한다.

스크롤 전 교집합이나 유효 후보가 없으면 leaf 요소에 `scrollIntoView`를 최대 한 번 호출하고 leaf와 모든 부모 geometry를 다시 측정한다. 같은 명령 안에서 부모 owner에 별도의 스크롤을 요청하지 않는다. 브라우저가 leaf 스크롤을 부모로 얼마나 전파하는지는 이 구현의 실제 E2E에서 확인해야 한다. 두 번째 준비도 실패하면 입력하지 않고 종료한다. 스크롤 자체는 페이지 상태를 바꿀 수 있으므로 포인터 미전송을 페이지 무변경으로 해석하지 않는다.

## hover 이후 원래 점 검증

선택한 leaf 점을 다시 hit test한 뒤 원래 root 점으로 Chromium `mouseMoved`를 전송한다. 입력 ACK를 받은 다음 순서는 다음과 같다.

1. 원래 leaf ref의 정확한 local 점을 다시 검사한다. 새 점을 고르거나 스크롤하지 않는다.
2. 같은 부모 owner의 backendNodeId와 context를 재확인하고 원래 local 점 하나를 부모마다 다시 변환한다. 부모 hit test도 반복한다.
3. 최상위까지 변환한 결과가 처음 `mouseMoved`에 사용한 root 점과 정확히 같아야 한다. 다른 root 점으로 입력을 보정하지 않는다.
4. leaf의 같은 점을 한 번 더 확인한다. `hover`는 여기서 완료하고, `click`만 원래 root 점에 `mousePressed`와 `mouseReleased`를 보낸다.

hover 성공은 `dispatchAcknowledged:true`, `pointVerified:true`를 반환한다. click 성공은 기존 [포인터 입력](pointer-input.md)의 엔진 ACK 계약을 따른다. hover나 click으로 메뉴가 열렸는지, 링크가 이동했는지, 서버 저장이 끝났는지는 별도로 다시 읽어 확인한다. `hover`는 버튼 누름이나 놓음을 전송하지 않는다.

원래 local 점과 root 점을 모두 고정하므로 레이아웃이 달라졌지만 같은 요소를 누를 수 있는 경우에도 보수적으로 실패할 수 있다. 그때 다른 후보나 새 좌표로 자동 재시도하지 않는다. hover 이후 실패는 이미 pointer movement가 접수됐을 수 있으며, 오류의 `attempted`와 `outcomeUnknown`을 확인해야 한다.

현재 CLI 오류는 내부 DOM helper 오류를 모두 그대로 전달하지 않는다. 부모 검사 단계의 실패는 `FRAME_OWNER_UNAVAILABLE`, leaf geometry 또는 candidates 준비 실패는 `FRAME_POINTER_UNAVAILABLE`로 반환한다. Rust clipping 계산 실패에는 `UNSUPPORTED_GEOMETRY`, 잘못된 좌표와 후보 응답에는 `INVALID_POINT`가 사용된다. 유효 후보가 없거나 hover 뒤 원래 점이 맞지 않으면 `CLICK_POINT_UNAVAILABLE`이다. 같은 geometry 원인이어도 실패한 단계에 따라 코드가 다를 수 있다.

## 지원 geometry와 남은 경합

부모 owner와 HTML composed ancestor의 양수 축 정렬 CSS scale, transform과 zoom을 모델링한다. `display:contents`에서는 박스 transform과 individual scale을 곱하지 않고 zoom은 유지한다. computed border box에 배율을 적용한 크기와 실제 bounding rect의 차이가 0.02 CSS px를 넘으면 지원하지 않는 geometry로 거절한다.

다음 조건은 추정 좌표로 진행하지 않는다.

- 부모 owner 경로의 회전, skew, 음수 배율, matrix3d, perspective, motion path, SVG 또는 foreignObject 기반 변환.
- leaf나 부모의 visual viewport scale이 1이 아니거나 offset이 0이 아닌 경우.
- 상한을 넘는 좌표, 해석할 수 없는 computed 크기나 변환, 현재 frame과 다른 owner 또는 context.

위 제한은 부모 좌표 변환 경로에 적용된다. leaf 안의 실제 타깃은 bounding rect 후보와 hit test로 확인하며, 사각형 전체가 클릭 가능한 영역이라고 가정하지 않는다. viewport 경계는 scrollbar 영역을 제외하고 standards와 quirks 문서의 root 차이를 반영한다.

최대 9개의 후보는 모든 부분 가림을 완전 탐색하지 않는다. 통과 가능한 작은 영역이 있어도 후보가 닿지 않으면 실패할 수 있다. 후보 검사와 부모별 측정은 서로 다른 renderer 요청이며 원자적 레이아웃 snapshot이 아니다. 최종 검사 뒤 mousePressed가 도착하기 전에도 페이지가 이동하거나 다른 요소가 덮을 수 있다. 이 경합을 잠그거나 사람 이벤트를 차단하지 않는다.

## 요청 수명과 메모리

DOM 준비와 검증은 원래 frame session으로, `Input.dispatchMouseEvent`는 root session으로 보낸다. raw 응답은 request ID, Browser와 정확한 session을 함께 대조한다. root pointer ACK를 child DOM 응답으로 소비하지 않는다.

Runtime 이벤트는 현재 처리 중인 부모뿐 아니라 같은 session에 속한 leaf와 모든 부모 `ContextTracker`에 반영한다. 문서나 context 파괴, frame 연결 변경, task 제어권 변경과 취소는 진행 중 요청을 무효화한다. 각 부모의 원격 owner는 object group 해제 ACK 뒤 정리하며 재검사 때 다시 얻는다. `TAB_BUSY`는 입력과 최종 Runtime disable ACK까지 유지한다.

Rust는 부모 최대 32개, geometry와 후보 최대 9개를 요청 수명 안에서 보관한다. DOM bridge는 새 강한 요소 캐시를 추가하지 않는다. `frameAct`는 binding을 먼저 확인한 뒤 pointer 단계에서 이전 press와 insert 관찰자를 정리한다. 오래된 binding의 요청으로 새 관찰자를 지우지 않는다. 기존 frame 약한 ref와 world 메타데이터 수명은 [프레임 읽기](frame-read.md)의 제한을 유지한다.

새 worker, UI renderer, 저장소와 반복 타이머는 추가하지 않는다. 이 상한은 전체 Chromium 메모리나 실행 시간의 상한을 뜻하지 않는다.

## 실행하지 않은 검증

통합 E2E에서 같은 출처와 교차 출처, OOPIF, 중첩 iframe, 열린 Shadow DOM의 owner, border와 padding, zoom과 양수 scale, 부분 가림과 부모 overlay를 확인해야 한다. 화면 밖 leaf의 한 번 스크롤, hover로 생긴 overlay와 이동, 원래 local 점과 root 점의 보존, context 교체와 취소, 배경 AI 작업 중 사용자 공간과 선택 보존도 포함한다.

현재 읽기 리뷰에서는 DOM 반환값과 Rust 파싱, clipping, 후보 index, root 입력과 child 검증 분리, 전체 parent context 추적, 스크롤 상한의 호출 흐름을 확인했다. 실제 클릭 결과나 가림 방어 성공을 관찰하지 않았다. 이후 빌드와 실행 결과는 [작업 기록](work-log.md)에 별도로 남긴다.
