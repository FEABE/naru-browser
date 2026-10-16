# CLI Enter 입력

## 동작과 응답

`lightbrowser task run ID REV press TAB REF Enter`는 최근 `read`의 참조를 사용한다. 현재 키 지원은 Enter 한 개다. Tab, 화살표, 조합키, IME 입력은 지원하지 않는다.

DOM bridge는 대상의 현재 문서, 참조 세대, 표시와 활성 상태를 확인하고 포커스를 요청한다. 이어서 실제 `activeElement`가 대상인지 확인한다. 열린 ShadowRoot에서는 host까지, 같은 출처 iframe에서는 부모 문서의 iframe까지 포커스 연결을 확인한다. 연결 탐색은 64단계로 제한한다. `document.hasFocus()`를 요구하거나 창 활성화, 공간 전환을 요청하지 않는다. 즉시 다른 요소가 포커스를 가져가면 `FOCUS_NOT_REACHED`로 거절한다.

준비에 성공하면 Rust가 같은 CEF 호스트에 `Input.dispatchKeyEvent`의 `keyDown`, `keyUp`을 보낸다. 두 요청은 응답을 기다리지 않고 같은 UI 처리 안에서 차례로 접수한다. keyDown에는 Enter 텍스트 `\r`을 포함한다. DOM의 합성 KeyboardEvent와 `form.requestSubmit()`는 실행하지 않으며 엔진 실패 때 이전 방식으로 재시도하지 않는다. 버튼 클릭, 줄바꿈, 기본 제출 버튼과 keydown 취소 처리는 Chromium이 결정한다.

최종 성공의 `page`는 다음 필드를 포함한다.

```json
{
  "ok": true,
  "action": "press",
  "key": "Enter",
  "input": "chromium",
  "attempted": true,
  "dispatchAcknowledged": true
}
```

이는 두 엔진 요청에 성공 응답이 왔다는 뜻이다. 원하는 요소의 실제 수신, 클릭, 줄바꿈, 제출 성공을 보증하지 않는다. 엔진이 이벤트를 처리하지 않고 응답하는 경우도 있으므로 `read`나 `wait`로 효과를 확인한다. 예전 `trusted:false`, `formSubmissionRequested` 필드는 제거한다. 이벤트의 신뢰값과 물리 장치 입력 여부를 응답으로 추정하지 않는다.

## 입력 중 변경과 한계

두 요청을 접수하고 결과를 모으는 동안 같은 탭의 작업 요청을 유지한다. 두 응답이 모두 오기 전에 성공을 반환하지 않는다. 호스트와 매개변수를 준비한 뒤, 각 엔진 호출 직전에 요청 활성 상태, 작업 제어권, 문서 세대와 탭 상태를 다시 확인한다. 첫 호출 중 제어권이나 문서가 바뀌면 나머지 키를 새 대상에 보내지 않는다. 이미 접수한 입력은 취소할 수 없으며 자동 반복하지 않는다.

`attempted`는 엔진 키 요청을 시도했는지 나타낸다. false여도 준비 중 focus 핸들러의 부작용이 없다는 뜻은 아니다. 입력 이후 오류, 연결 종료나 제어권 상실은 `outcomeUnknown`과 함께 페이지를 다시 확인해야 하는 상황으로 다룬다. 문서나 제어권이 바뀐 뒤 keyUp을 보내지 않으면 이전 페이지의 키 처리 상태가 남을 수 있다. 같은 요청을 무조건 반복해 이를 복구하지 않는다.

DOM 확인과 엔진 입력은 서로 다른 처리이므로 원자적이지 않다. 확인 직후의 포커스 변경, 서브프레임 교체, keydown 핸들러가 일으킨 대상 변경까지 막는 장치가 아니다. 특히 핸들러가 포커스를 옮기면 같은 Enter의 후속 이벤트도 새 포커스로 갈 수 있다. 같은 출처 iframe과 열린 Shadow DOM만 참조로 찾을 수 있으며 교차 출처 iframe과 닫힌 Shadow DOM은 이 기능으로 확장하지 않는다.

`nativeVirtualKeyCode`는 0으로 보낸다. 설치된 Chromium 버전의 [Input 처리 구현](https://github.com/chromium/chromium/blob/156.0.8078.17/content/browser/devtools/protocol/input_handler.cc)은 이 경우 macOS 이벤트를 만들지 않고, 처리되지 않은 키를 브라우저 UI로 넘기지 않도록 설정한다. 같은 버전의 [렌더러 포커스 구현](https://github.com/chromium/chromium/blob/156.0.8078.17/content/browser/renderer_host/render_widget_host_impl.cc)은 페이지 포커스를 변경한다. 페이지의 focus 이벤트까지 발생하지 않는다는 뜻은 아니다. Naru는 네이티브 창이나 사용자 공간 선택 명령을 추가하지 않는다.

## 검증 범위

검증 결과와 정리는 [작업 기록](work-log.md)에 남긴다. 모의 DOM은 참조와 포커스 준비를, Rust 단위 검사는 요청과 응답의 수명주기를 검증한다. 별도 ego Chromium의 기본 키 동작 결과는 Naru CEF의 통합 성공으로 해석하지 않는다. ego의 공개 CDP 호출은 대상 탭을 자동 활성화하므로 배경 탭 선택 보존의 증거도 아니다.

`tests/native_press_smoke.py`는 실제 Naru에서 버튼, 줄바꿈, 제출 버튼, 취소, 포커스 거절, Shadow DOM과 iframe, 배경 AI 입력 중 사용자 선택 보존을 확인하는 후속 검사다. 앱을 실행하지 않은 구문 검사는 해당 통합 검사의 성공으로 기록하지 않는다. 외부 사이트, 사용자 입력과의 경합, 장시간 사용과 실제 메모리 측정도 별도 검증이 필요하다.
