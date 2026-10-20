# CLI 클릭과 마우스 올리기

더블클릭, 오른쪽 클릭, 휠, 끌기, 좌표 클릭·이동과 `text=`/`css=`/`role=` 선택자 대상은 같은 경로를 확장한 것이다. [포인터 확장](pointer-ext.md)을 따른다.

## 참조와 포인터 준비

`lightbrowser task run ID REV click TAB REF`는 최근 `read` 또는 최신 `read-frame`에서 받은 참조를 사용한다. DOM의 `.click()` 대신 Chromium 포인터 입력을 요청한다. `hover TAB REF`는 같은 참조 준비와 위치 확인을 사용하되 포인터 이동만 수행한다. 지원되는 텍스트 fill·insert는 별도의 Chromium 입력 경로를 사용하며 check와 select는 DOM 동작을 유지한다. 기존 `read`의 접근 가능한 문서 참조와 `read-frame`의 exact session/context 참조는 수명과 준비 경로를 구분한다.

먼저 참조와 문서, 표시 상태를 확인하고 viewport 안의 제한된 후보에서 실제 hit test로 대상을 찾는다. 기존 `read` 경로는 필요하면 대상과 접근 가능한 부모 iframe을 스크롤한다. `read-frame` 경로는 아래처럼 leaf를 한 번 스크롤한 뒤 부모 기하를 다시 수집한다. 대상 자체와 비대화형 자식은 허용하지만, 대상 아래의 다른 버튼이나 링크를 대신 누르지 않는다. 스크롤은 페이지의 scroll 핸들러를 실행할 수 있으며 입력을 보내기 전부터 페이지 상태가 바뀔 수 있다.

위치를 얻으면 `Input.dispatchMouseEvent`의 mouseMoved를 요청한다. 응답 뒤 원래 참조와 정확히 같은 위치를 다시 확인한다. 이 재검사는 스크롤하거나 다른 점을 고르지 않는다. 통과한 위치에 왼쪽 mousePressed와 mouseReleased를 차례로 보내며, 다른 대상이나 새 문서로 자동 재시도하지 않는다. hover는 같은 위치의 재검사를 통과하면 종료한다. mousePressed와 mouseReleased를 보내지 않는다. 준비부터 최종 응답까지 [임시 페이지 포커스 준비](focus-input.md#포인터-입력)를 유지한다. 숨은 상태에서 로드되어 그려지지 않은 작업 탭은 이 설정이 없으면 버튼 입력이 페이지에 전달되지 않는다. 대신 동작 중 페이지에 focus, blur, visibilitychange가 생길 수 있다. 포인터 이동의 효과로 메뉴가 열리면 새 read로 메뉴 항목 참조를 얻은 뒤 별도 click을 요청한다. DOM 검사와 엔진 입력 사이의 페이지 변경을 원자적으로 막는 기능은 아니다.

## 좌표와 지원 범위

엔진에 보내는 포인터 좌표는 root viewport의 CSS pixel이다. 기존 `read` 경로는 열린 Shadow DOM과 접근 가능한 같은 출처 iframe을 따라가며, 프레임마다 내용 영역과 부모의 hit test를 확인한다. `read-frame` 경로는 원래 leaf CSS 좌표와 root 입력 좌표를 별도로 보관한다. iframe의 양수 축 정렬 배율, border와 padding을 반영하고 계산한 크기를 실제 사각형과 대조한다. `display: contents` 조상의 박스 변환은 제외하고 zoom은 반영한다. 프레임 좌표 변환 경로의 회전, 기울임, perspective, motion path와 SVG 조상처럼 정확한 변환을 확인하지 못한 경우는 거절한다. 문서 내부의 회전 요소는 실제 hit test를 통과한 후보가 있을 때 처리할 수 있다.

후보 생성 한 번당 최대 9개이며, 스크롤이 필요하면 한 번 더 생성하므로 두 회 합계는 최대 18개다. 이는 부모 hit test와 고정점 사후 재검사의 횟수 전체를 뜻하지 않는다. 기존 `read`는 iframe 중첩 8단계, shadow와 조상 탐색 64단계를 사용한다. `read-frame` 포인터는 inventory의 실제 부모 연결 최대 32개를 따라가며 각 문서에서 조상 탐색 64단계를 유지한다. 전체 사각형 목록이나 DOM 사본을 만들지 않는다. 작은 노출 영역이 후보에 걸리지 않으면 클릭하지 않는다. 스크롤바 영역은 후보에서 제외한다. 확대나 offset이 있는 visualViewport와 닫힌 Shadow DOM 내부는 지원하지 않는다. 교차 출처·OOPIF의 참조는 별도 `read-frame`으로 얻는다.

## 교차 출처·OOPIF frame ref

v8 소스의 `read-frame`은 `supportedActions` 일곱 개와 `pointerActionsSupported:true`를 반환한다. 이는 포인터 경로의 존재를 뜻하며 읽은 모든 요소가 클릭 가능한 상태라는 뜻은 아니다. frame snapshot은 원래 Browser·owner/task revision·navigation/document 세대·frame/session·unique context·documentId/generation과 원래 ref에 묶인다. 새 read나 다른 context로 대상을 바꿔 재시도하지 않는다.

처음에는 leaf의 rect와 viewport를 얻는다. 부모마다 정확한 session/context에서 `DOM.getFrameOwner`로 원래 child의 owner를 찾고, `DOM.resolveNode`와 `Runtime.callFunctionOn`의 owner 인수로 content mapping·viewport를 읽는다. Rust는 이 영역들을 leaf 좌표로 합성해 공유 clip을 만들고 leaf의 최대 9개 후보를 구한다. 이후 부모별 현재 hit test에서 원래 owner에 닿는 후보만 부모 좌표로 옮긴다. 후보 index와 변환·범위를 Rust에서도 검사하며, 최종 후보의 leaf/root 좌표 한 쌍을 고정한다.

clip이 비거나 유효 후보가 없으면 포인터를 보내기 전에 leaf만 `scrollIntoView({behavior:'instant',block:'center',inline:'center'})`로 한 번 스크롤하고 leaf·부모 geometry부터 모두 다시 수집한다. parent owner 전체를 스크롤한 것만으로 큰 iframe 안의 특정 leaf가 노출됐다고 추정하지 않는다. 두 번째에도 후보가 없거나 지원하지 않는 기하이면 실패한다.

포인터 전송은 child session을 쓰지 않고 root session에 root CSS 좌표를 보낸다. DOM 준비·확인은 원래 child unique context를 유지한다. mouseMoved ACK 후에는 leaf 동일점 → 전체 부모 owner hit test와 root 좌표 변환 → 원래 root point와의 정확한 일치 → leaf 동일점 순서로 확인한다. hover는 여기서 성공 응답을 준비하고 click만 같은 root point로 버튼 배치를 보낸다. 이동 후에는 스크롤·다른 점 선택·포인터 재전송으로 보정하지 않는다.

부모 한 곳의 조회를 마칠 때마다 owner objectGroup release ACK와 Runtime disable ACK를 확인한다. 같은 session의 context 이벤트는 대상과 모든 부모 tracker에 적용하므로 여러 world를 순차 조회하는 동안 앞서 확인한 world의 변경도 무효화에 반영한다. 명령의 마지막 대상 Runtime disable ACK까지 탭 잠금과 요청 수명을 유지한다. [프레임 포인터 상세](frame-pointer.md)와 [연결·정리](devtools-routing.md)를 따른다.

고정 Chromium의 마우스 라우팅은 session의 local-root widget에서 시작해 hit-test 결과의 widget 좌표로 변환한다. box/quads의 local-root 좌표를 OOPIF 전체 root 좌표로 가정하지 않으며, CSS point에 devicePixelRatio를 곱하지 않는다. 일반 browser zoom과 device scale은 엔진이 처리한다. leaf scrollIntoView는 일반 OOPIF 부모로 전달되지만 원격 IPC와 compositor 갱신은 비동기여서 호출 반환이 완료 barrier가 아니다. 이 설명은 [고정 엔진 소스](../artifacts/frame-pointer-v1-sources.json)에 근거하며 Naru 실행 결과가 아니다.

## 응답의 의미

최종 성공은 `action:"click"`, `input:"chromium"`, `attempted:true`, `dispatchAcknowledged:true`를 반환한다. 이는 엔진 요청의 접수와 응답을 확인했다는 뜻이다. 실제 클릭 핸들러, 탐색, 다운로드나 제출의 성공은 `read`, `wait`와 해당 상태로 따로 확인한다. 응답에 이벤트의 trusted 값이나 물리 마우스 입력 여부를 추정해 넣지 않는다.

hover 성공은 `action:"hover"`, `input:"chromium"`, `attempted:true`, `dispatchAcknowledged:true`, `pointVerified:true`다. pointVerified는 원래 위치의 사후 hit test를 뜻하며 frame ref에서는 leaf와 부모 연결의 검사를 포함한다. 메뉴 표시 완료나 일정 시간의 머무름을 보장하지 않는다. 메뉴가 원래 대상을 덮거나 이동시키면 오류를 반환할 수 있다. 이 경우 자동 재이동이나 클릭을 하지 않고 다시 읽는다. `CLICK_POINT_UNAVAILABLE` 등 기존 포인터 오류 코드도 hover에 적용하되 응답 action은 hover를 유지한다.

설치된 Chromium 156.0.8078.17의 [InputHandler](https://github.com/chromium/chromium/blob/156.0.8078.17/content/browser/devtools/protocol/input_handler.cc)는 CSS 좌표를 엔진 배율에 맞춰 전달하고 대상 페이지에 포커스를 요청한다. 입력이 큐에 들어가지 않거나 정리되는 경로에도 성공 응답이 존재한다. Naru가 창 활성화나 공간 선택 명령을 추가하지 않아도 페이지의 hover와 focus 효과는 생길 수 있다. renderer가 버린 입력도 이 응답은 성공이므로 숨은 탭 전달은 위 임시 설정에 의존한다.

`attempted`는 엔진 포인터 요청을 시도했는지를 나타낸다. 이동 요청 이후 클릭 전 검사에서 실패해도 true다. false여도 준비 중 스크롤의 부작용이 없었다는 뜻은 아니다. 오류의 `outcomeUnknown`을 확인하고 같은 클릭을 무조건 반복하지 않는다.

포인터 이동부터 재검사와 버튼 입력까지 같은 탭의 요청을 유지한다. 각 제출 직전 요청 활성 상태, 작업 제어권, 원래 브라우저와 문서 세대를 재검사한다. 키 입력과 고정 크기의 요청 배치 처리를 공유하며 추가 renderer, worker, 타이머나 저장 파일을 만들지 않는다. 실제 RAM 측정 결과는 아니다.

## 남는 경합과 검증

mouseMoved의 응답이 모든 hover 효과의 완료를 보증하지 않는다. 원격 부모의 스크롤·compositor hit-test 반영, 재검사 직후의 이동과 가림, mousedown·지연 focus 핸들러가 바꾸는 대상, iframe 교체와 사용자 입력 경합은 남는다. 중간 취소나 문서 변경으로 mouseReleased를 보내지 못하면 이전 페이지의 버튼 상태가 남을 수 있다. 자동 release나 클릭 재전송으로 보정하지 않는다.

Chromium 입력의 기본 동작은 과거 합성 click과 다를 수 있다. 사용자 활성화가 필요한 팝업, 다운로드와 파일 선택은 실제 Naru에서 별도 확인해야 한다. 기존 합성 click 기반 권한 검사 결과를 이 버전의 성공 증거로 재사용하지 않는다.

확인한 다운로드 동작은 다음과 같다. CLI click의 mouseDown은 Chromium 자동 다운로드 제한기에 사용자 상호작용으로 집계된다. click 한 번마다 다운로드 한 건이 허용된다. 한 번의 click에서 스크립트가 여러 다운로드를 시작하면 첫 건 뒤의 요청은 권한 승인을 기다린다. 모든 다운로드는 여전히 사람이 저장 위치를 고른 뒤에 시작한다. 2026-10-19에 사용자가 이 Chromium 동작을 유지하기로 정했다. `tests/download_permissions_smoke.py`는 이 동작 기준으로 fixture 버튼 한 번의 click에서 두 다운로드를 시작해 검사한다.

모의 DOM은 위치 준비와 거절 조건을, Rust 단위 검사는 요청과 응답의 수명을 확인한다. 독립 ego Chromium의 자체 fixture 결과는 Naru CEF의 배경 입력, 네이티브 선택 보존과 실제 macOS 입력의 증거가 아니다. 준비된 `tests/native_click_smoke.py`는 hover로 메뉴가 열려도 버튼 누름과 클릭은 0회인지, 후속 read의 새 ref로 선택이 한 번만 일어나는지, 사용자 공간과 선택이 유지되는지 확인한다. 실행 전에는 통합 성공으로 기록하지 않는다. 실행 결과와 미검증 범위는 [작업 기록](work-log.md)에 구분한다.

이전 frame ref 다섯 입력의 [v7 통합 빌드](../artifacts/ego-parity-v7-build.json)는 exit 0이었다. 이번 frame click·hover의 [v8 통합 빌드](../artifacts/ego-parity-v8-build.json)도 exit 0, 91.49초로 완료했고 제품 소스·Cargo 35개의 전후 해시가 일치했다. 단위·모의 DOM·앱·CEF 검사는 실행하지 않았다. source 단계 연결과 upstream 근거를 실제 cross-origin/OOPIF 포인터 성공으로 대신하지 않는다.
