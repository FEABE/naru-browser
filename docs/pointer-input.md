# CLI 클릭 입력

## 참조와 포인터 준비

`lightbrowser task run ID REV click TAB REF`는 최근 `read`에서 받은 참조를 사용한다. DOM의 `.click()` 대신 Chromium 포인터 입력을 요청한다. `check`의 상태 지정과 fill, select는 기존 DOM 동작을 유지한다.

먼저 참조와 문서, 표시 상태를 확인하고 필요하면 대상과 부모 iframe을 스크롤한다. viewport 안의 제한된 후보에서 실제 hit test로 대상을 찾는다. 대상 자체와 비대화형 자식은 허용하지만, 대상 아래의 다른 버튼이나 링크를 대신 누르지 않는다. 스크롤은 페이지의 scroll 핸들러를 실행할 수 있으며 입력을 보내기 전부터 페이지 상태가 바뀔 수 있다.

위치를 얻으면 `Input.dispatchMouseEvent`의 mouseMoved를 요청한다. 응답 뒤 원래 참조와 정확히 같은 위치를 다시 확인한다. 이 재검사는 스크롤하거나 다른 점을 고르지 않는다. 통과한 위치에 왼쪽 mousePressed와 mouseReleased를 차례로 보내며, 다른 대상이나 새 문서로 자동 재시도하지 않는다. DOM 검사와 엔진 입력 사이의 페이지 변경을 원자적으로 막는 기능은 아니다.

## 좌표와 지원 범위

좌표는 메인 문서 viewport의 CSS pixel이다. 열린 Shadow DOM과 접근 가능한 같은 출처 iframe을 따라가며, 프레임마다 내용 영역과 부모의 hit test를 확인한다. iframe의 양수 축 정렬 배율, border와 padding을 반영하고 계산한 크기를 실제 사각형과 대조한다. `display: contents` 조상의 박스 변환은 제외하고 zoom은 반영한다. 프레임 좌표 변환 경로의 회전, 기울임, perspective, motion path와 SVG 조상처럼 정확한 변환을 확인하지 못한 경우는 거절한다. 메인 문서의 회전 요소는 실제 hit test를 통과한 후보가 있을 때 처리할 수 있다.

위치 검사 한 번당 후보는 최대 9개이며, 스크롤이 필요하면 한 번 더 검사한다. iframe은 기존 중첩 8단계, shadow와 조상 탐색은 64단계를 사용한다. 전체 사각형 목록이나 DOM 사본을 만들지 않는다. 작은 노출 영역이 후보에 걸리지 않으면 클릭하지 않는다. 스크롤바 영역은 후보에서 제외한다. 확대나 offset이 있는 visualViewport는 지원하지 않으며 교차 출처 iframe과 닫힌 Shadow DOM 내부를 새로 관찰하지 않는다.

## 응답의 의미

최종 성공은 `action:"click"`, `input:"chromium"`, `attempted:true`, `dispatchAcknowledged:true`를 반환한다. 이는 엔진 요청의 접수와 응답을 확인했다는 뜻이다. 실제 클릭 핸들러, 탐색, 다운로드나 제출의 성공은 `read`, `wait`와 해당 상태로 따로 확인한다. 응답에 이벤트의 trusted 값이나 물리 마우스 입력 여부를 추정해 넣지 않는다.

설치된 Chromium 156.0.8078.17의 [InputHandler](https://github.com/chromium/chromium/blob/156.0.8078.17/content/browser/devtools/protocol/input_handler.cc)는 CSS 좌표를 엔진 배율에 맞춰 전달하고 대상 페이지에 포커스를 요청한다. 입력이 큐에 들어가지 않거나 정리되는 경로에도 성공 응답이 존재한다. Naru가 창 활성화나 공간 선택 명령을 추가하지 않아도 페이지의 hover와 focus 효과는 생길 수 있다.

`attempted`는 엔진 포인터 요청을 시도했는지를 나타낸다. 이동 요청 이후 클릭 전 검사에서 실패해도 true다. false여도 준비 중 스크롤의 부작용이 없었다는 뜻은 아니다. 오류의 `outcomeUnknown`을 확인하고 같은 클릭을 무조건 반복하지 않는다.

포인터 이동부터 재검사와 버튼 입력까지 같은 탭의 요청을 유지한다. 각 제출 직전 요청 활성 상태, 작업 제어권, 원래 브라우저와 문서 세대를 재검사한다. Enter와 고정 크기의 요청 배치 처리를 공유하며 추가 renderer, worker, 타이머나 저장 파일을 만들지 않는다. 실제 RAM 측정 결과는 아니다.

## 남는 경합과 검증

mouseMoved의 응답이 모든 hover 효과의 완료를 보증하지 않는다. 재검사 직후의 이동과 가림, mousedown 핸들러가 바꾸는 대상, iframe 교체와 사용자 입력 경합은 남는다. 중간 취소나 문서 변경으로 mouseReleased를 보내지 못하면 이전 페이지의 버튼 상태가 남을 수 있다. 자동 release나 클릭 재전송으로 보정하지 않는다.

Chromium 입력의 기본 동작은 과거 합성 click과 다를 수 있다. 사용자 활성화가 필요한 팝업, 다운로드와 파일 선택은 실제 Naru에서 별도 확인해야 한다. 기존 합성 click 기반 권한 검사 결과를 이 버전의 성공 증거로 재사용하지 않는다.

모의 DOM은 위치 준비와 거절 조건을, Rust 단위 검사는 요청과 응답의 수명을 확인한다. 독립 ego Chromium의 자체 fixture 결과는 Naru CEF의 배경 입력, 네이티브 선택 보존과 실제 macOS 입력의 증거가 아니다. 준비된 `tests/native_click_smoke.py`도 실행 전에는 통합 성공으로 기록하지 않는다. 실행 결과와 미검증 범위는 [작업 기록](work-log.md)에 구분한다.
