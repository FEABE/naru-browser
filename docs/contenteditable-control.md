# HTML 편집 영역의 Chromium 텍스트 입력

후속 소스의 `fill TAB REF TEXT`는 명시적인 HTML `contenteditable` 영역의 내용을 선택한 뒤 Chromium `Input.insertText`로 교체한다. 합성 beforeinput과 input, 직접 DOM 자식 교체를 제거했다. 구현과 최신 Naru 앱 실행 결과는 구분하며 통합 빌드 후 [E2E 목록](ego-parity-e2e.md)으로 확인한다.

## 지원 범위

`contenteditable`이 빈 속성, true 또는 plaintext-only이고 실제 isContentEditable도 true인 HTML 요소를 사용한다. 부모가 편집 가능한 중첩 요소와 상속만 받은 자식은 자동으로 부모에 연결하지 않는다. 문서 designMode는 지원하지 않는다. 열린 Shadow DOM과 접근 가능한 같은 출처 iframe은 기존 참조 문맥을 사용한다.

요청은 영역 전체를 일반 텍스트로 교체하는 동작이다. 기존 서식, 링크와 이미지의 보존을 보장하지 않는다. 임의 웹 편집기의 내부 모델이나 서버 저장을 검증하는 기능도 아니다. 실제 한글 IME 입력을 재현하지 않는다.

## 입력과 결과

1. 원래 참조와 문서, 표시, 편집 가능 상태와 aria-readonly를 확인한다.
2. 대상을 focus하고 Range와 Selection으로 전체 내용을 선택한다. 원래 영역의 선택과 포커스 연결을 확인한다.
3. 원래 요청 문자열로 Input.insertText를 한 번 보낸다. beforeinput 취소와 편집, 이벤트와 기본 undo 처리는 Chromium에 맡긴다.
4. 같은 문서와 참조, 편집 상태와 포커스를 확인한 뒤 네이티브 innerText를 요청 문자열과 비교한다. 줄바꿈이나 공백을 임의로 정규화하지 않는다.

성공은 `input:chromium`, `attempted:true`, `dispatchAcknowledged:true`, `verified:true`다. 이는 확인 시점의 텍스트 일치이며 편집기의 내부 데이터나 저장 완료를 뜻하지 않는다. 이전 DOM 경로의 changed와 trusted:false 응답은 이 경로에서 사용하지 않는다. 값이 이미 같아도 별도의 무동작 성공으로 바꾸지 않는다.

선택과 포커스 준비 실패는 입력 전 오류다. 입력 뒤 원래 대상을 확인할 수 없으면 FILL_STATE_UNCONFIRMED, 텍스트가 다르면 FILL_STATE_NOT_REACHED다. 취소 핸들러가 값을 유지하거나 바꾸는 동작도 최종 결과로 판단한다. 오류 뒤 자동 재시도와 DOM 방식의 대체 입력은 없다.

DOM 준비와 엔진 입력은 원자적이지 않다. 준비 직후 다른 요소로 포커스가 이동하면 이미 발생한 변경을 사후 확인으로 취소할 수 없다. 후속 비동기 편집은 다시 read해서 확인해야 한다.

## 자원과 검증

네이티브 innerText를 읽기 전에 노드 수와 깊이, 텍스트 크기를 제한한 순회를 한다. 한계를 넘거나 프레임과 shadow 내용 때문에 확인 범위가 불명확하면 결과를 확정하지 않는다. 전체 innerHTML이나 textContent를 무제한으로 복제하지 않는다. 요청 문자열은 기존 프로토콜 크기 제한을 따르며 입력값과 페이지 예외 원문을 오류 응답에 넣지 않는다.

단일 줄, 여러 줄, 빈 문자열, rich host와 plaintext-only, beforeinput 취소, DOM 교체, iframe과 Shadow DOM, 배경 사용자 선택 보존을 같은 앱에서 확인해야 한다. 이번 구현에 이전 합성 DOM 검사의 통과 결과를 적용하지 않는다. 실제 실행, 미실행 이유와 생성물 정리는 [작업 기록](work-log.md)에 남긴다.
