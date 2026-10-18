# CLI 키 입력과 포커스 관찰

## 동작과 응답

`lightbrowser task run ID REV press TAB REF KEY`는 최근 `read`의 참조를 사용한다. 키 이름은 대소문자까지 정확히 지정한다.

| 구분 | 지원 이름 |
| --- | --- |
| 기본 키 14개 | Enter, Tab, Escape, Space, ArrowLeft, ArrowUp, ArrowRight, ArrowDown, Home, End, PageUp, PageDown, Backspace, Delete |
| Shift 조합 10개 | Shift+Enter, Shift+Tab, Shift+ArrowLeft, Shift+ArrowUp, Shift+ArrowRight, Shift+ArrowDown, Shift+Home, Shift+End, Shift+PageUp, Shift+PageDown |
| macOS 편집 조합 18개 | Meta+A, Meta+Z, Meta+Shift+Z, Meta+ArrowLeft/Up/Right/Down, Meta+Shift+ArrowLeft/Up/Right/Down, Meta+Backspace, Alt+ArrowLeft/Right, Alt+Shift+ArrowLeft/Right, Alt+Backspace, Alt+Delete |

위 표는 총 42개 이름이다. Meta는 macOS의 Command, Alt는 Option에 해당한다. 임의 문자, Return과 Esc 같은 별칭, Ctrl, 표에 없는 조합과 IME는 지원하지 않는다. Meta+Delete와 클립보드 C, V, X도 미지원이다. 전체 문자열 교체는 `fill`, 현재 커서나 선택 영역의 입력은 [insert](insert-text.md)를 사용한다. CLI와 브라우저가 모두 키를 검증하고, 브라우저는 포커스 준비 응답이 원래 요청한 키와 일치해야 전송한다.

DOM bridge는 대상의 현재 문서, 참조 세대, 표시와 활성 상태를 확인하고 포커스를 요청한다. 이어서 실제 `activeElement`가 대상인지 확인한다. 열린 ShadowRoot에서는 host까지, 같은 출처 iframe에서는 부모 문서의 iframe까지 포커스 연결을 확인한다. 연결 탐색은 64단계로 제한한다. `document.hasFocus()`를 요구하거나 창 활성화, 공간 전환을 요청하지 않는다. 즉시 다른 요소가 포커스를 가져가면 `FOCUS_NOT_REACHED`로 거절한다.

후속 소스는 이 준비 앞에 [임시 페이지 포커스 준비](focus-input.md)를 둔다. 정확한 session의 enable ACK 뒤 원 ref의 준비를 새로 실행하고, 기존 키 입력 한 쌍이 끝나면 disable ACK 뒤 keydown 관찰을 읽기 전용으로 소비한다. 사용자 창의 first responder를 바꾸는 CEF set_focus를 호출하지 않으며 실제 AppKit 포커스 보존은 E2E에서 확인한다. root ACK가 모든 OOPIF의 포커스 처리를 끝냈다는 뜻은 아니다.

준비에 성공하면 Rust가 같은 CEF 호스트에 `Input.dispatchKeyEvent` 요청 두 개를 보낸다. Enter와 Space는 `keyDown`에 각각 `\r`과 공백을 넣고, 나머지는 텍스트 없는 `rawKeyDown`을 사용한다. 마지막은 모두 텍스트 없는 `keyUp`이다. 두 요청은 응답을 기다리지 않고 같은 UI 처리 안에서 차례로 접수한다. modifier는 Alt 1, Meta 4, Shift 8의 비트합으로 두 이벤트에 넣는다. 별도 modifier 누름과 해제 이벤트나 지속적인 키 상태는 만들지 않는다. Space의 DOM key는 공백이고 code는 Space이며 CLI 응답은 요청 이름을 유지한다.

DOM의 합성 KeyboardEvent와 `form.requestSubmit()`는 실행하지 않으며 엔진 실패 때 이전 방식으로 재시도하지 않는다. 버튼 활성화, 줄바꿈, 제출, 포커스와 선택 이동, 삭제와 keydown 취소 처리는 Chromium이 결정한다. 기본 키 24개는 기존 Chromium 기본 처리를 유지한다. 새 편집 조합 18개는 macOS 키 바인딩을 nativeVirtualKeyCode 0에서 추정하지 않고 keydown에 한 개의 Blink editing command를 명시한다. keyup에는 commands를 넣지 않는다. 전체 선택, 실행 취소와 다시 실행, 줄과 문서 경계, 단어 이동과 선택, 삭제를 지원하며 실제 편집 효과와 preventDefault 존중 여부는 통합 E2E로 확인한다. PageUp과 PageDown의 결과는 컨트롤과 플랫폼에 따라 다르며 항상 캐럿을 움직인다고 보장하지 않는다. 매핑은 고정 Chromium 버전의 [Input 프로토콜](https://github.com/chromium/chromium/blob/156.0.8078.17/third_party/blink/public/devtools_protocol/domains/Input.pdl), [키 코드](https://github.com/chromium/chromium/blob/156.0.8078.17/ui/events/keycodes/keyboard_codes_posix.h), [편집 기본 동작](https://github.com/chromium/chromium/blob/156.0.8078.17/third_party/blink/renderer/core/editing/editing_behavior.cc)을 기준으로 한다.

편집 조합 매핑의 근거는 공식 Playwright의 macEditingCommands와 Chromium 입력 구현이다. [조회 기록](../artifacts/mac-editing-commands-v1-sources.json)에 URL, 고정 Chromium 자료와 조회 범위를 남겼다. 움직이는 main 소스의 매핑 확인은 Naru CEF의 실행 성공을 뜻하지 않는다.

최종 성공의 `page`는 다음 필드를 포함한다.

```json
{
  "ok": true,
  "action": "press",
  "key": "Enter",
  "input": "chromium",
  "attempted": true,
  "dispatchAcknowledged": true,
  "keydownTarget": "matched"
}
```

이는 두 엔진 요청에 성공 응답이 왔다는 뜻이다. 원하는 요소의 실제 수신, 클릭, 줄바꿈, 제출 성공을 보증하지 않는다. 엔진이 이벤트를 처리하지 않고 응답하는 경우도 있으므로 `read`나 `wait`로 효과를 확인한다. 예전 `trusted:false`, `formSubmissionRequested` 필드는 제거한다. 이벤트의 신뢰값과 물리 장치 입력 여부를 응답으로 추정하지 않는다.

## 키다운 대상 관찰

후속 소스는 포커스 준비 직후 원래 문서에 수동 관찰자를 등록한다. 지원 키와 modifier가 일치하는 trusted keydown을 관찰하고 두 엔진 응답 뒤 원래 isolated context에서 결과를 소비한다. 관찰은 이벤트를 취소하거나 전달을 차단하지 않는다. Tab 뒤 포커스 이동이나 keydown 핸들러가 원래 요소를 제거하는 정상 동작을 막지 않도록 keyup 대상과 사후 포커스 일치를 요구하지 않는다.

`keydownTarget`은 `matched`, `mismatched`, `unconfirmed` 중 하나다. 다른 요소로 전달된 관찰은 `KEY_TARGET_MISMATCH`, `attempted:true`, `outcomeUnknown:true` 오류로 반환한다. `matched`는 관찰한 키다운 대상만 확인한 값이며 그 뒤의 제출, 편집 결과나 서버 저장 성공을 뜻하지 않는다. 동시에 사람이 같은 키를 입력하면 CDP 요청과 완벽하게 구분할 수 없다. 관찰을 얻지 못한 경우에는 `unconfirmed`로 표시하고 성공으로 추정하지 않는다. 엔진 접수 자체의 성공과 대상 확인 여부는 별도 필드다.

Enter가 탐색을 일으켜 원래 문서가 바뀌었으면 두 엔진 응답의 성공은 유지하되 대상 관찰은 `unconfirmed`다. 관찰로 전환하거나 응답을 기다리는 도중 원래 문서가 탐색으로 바뀐 경우에도 unconfirmed로 처리한다. 제어권 변경, 취소, 렌더러 종료와 Browser 교체는 기존 오류를 유지한다. 어느 경우에도 자동 재시도하지 않는다. 이 기능은 지연된 포커스 이동으로 잘못 입력한 사실을 드러내는 보완이며, 이미 발생한 입력을 방지하거나 되돌리는 해결책은 아니다.

한 bridge에 관찰자 하나만 유지한다. 새 읽기나 동작, 결과 소비 또는 5초의 단발 만료에서 해제한다. Event 객체와 입력값은 저장하지 않는다. 반복 타이머나 별도 프로세스는 없다. 이번 변경의 실제 Naru 동작은 통합 빌드 후 [E2E 목록](ego-parity-e2e.md)의 입력 여정으로 확인한다.

## Tab 이후 다음 대상 찾기

`read`는 이번에 반환한 interactive 항목 중 논리 DOM 포커스 대상을 `focusedRef`로 함께 반환한다. 같은 읽기의 요소 참조와 동일한 탐색 세대 접두사를 사용한다. 다음 키를 현재 포커스에 보내려면 새 `focusedRef`를 사용한다. 각 `press`는 지정한 참조에 다시 포커스를 요청하므로 같은 원래 참조에 Tab을 반복하면 연속 순회가 되지 않는다.

열린 Shadow DOM과 접근 가능한 iframe의 가장 안쪽 대상을 기존 읽기 범위 안에서 확인한다. 반환된 참조가 없거나 읽기 상한 밖인 경우, 접근 불가 프레임, 포커스가 관찰 중 바뀐 경우에는 null이다. 닫힌 Shadow DOM 내부 포커스와 직접 host 포커스를 공개 DOM만으로 구별할 수 없다. 따라서 열린 root가 없는 shadow host 가능 요소는 div와 custom element, 일부 contenteditable을 포함해 null로 반환한다. null은 페이지나 앱에 포커스가 없다는 확정값이 아니다.

이 필드는 읽기 중의 관찰이며 원자적 스냅샷이나 OS 창 포커스가 아니다. 관찰은 focus 호출이나 창 활성화를 하지 않는다. Tab은 정상적으로 다음 대상으로 이동하며 keyup도 새 대상에 전달될 수 있다. 원래 대상이 포커스를 유지하지 않았다는 이유만으로 입력 결과를 거절하지 않는다. 마지막 요소의 Tab과 첫 요소의 Shift+Tab에서 실제 Naru 셸 포커스가 어떻게 이어지는지는 후속 통합 검증 범위다.

## 입력 중 변경과 한계

두 요청을 접수하고 결과를 모으는 동안 같은 탭의 작업 요청을 유지한다. 두 응답이 모두 오기 전에 성공을 반환하지 않는다. 호스트와 매개변수를 준비한 뒤, 각 엔진 호출 직전에 요청 활성 상태, 작업 제어권, 문서 세대와 탭 상태를 다시 확인한다. 첫 호출 중 제어권이나 문서가 바뀌면 나머지 키를 새 대상에 보내지 않는다. 이미 접수한 입력은 취소할 수 없으며 자동 반복하지 않는다.

`attempted`는 엔진 키 요청을 시도했는지 나타낸다. false여도 준비 중 focus 핸들러의 부작용이 없다는 뜻은 아니다. 입력 이후 오류, 연결 종료나 제어권 상실은 `outcomeUnknown`과 함께 페이지를 다시 확인해야 하는 상황으로 다룬다. 문서나 제어권이 바뀐 뒤 keyUp을 보내지 않으면 이전 페이지의 키 처리 상태가 남을 수 있다. 같은 요청을 무조건 반복해 이를 복구하지 않는다.

DOM 확인과 엔진 입력은 서로 다른 처리이므로 원자적이지 않다. 확인 직후의 포커스 변경, 서브프레임 교체, keydown 핸들러가 일으킨 대상 변경까지 막는 장치가 아니다. 핸들러나 기본 동작이 포커스를 옮기면 후속 이벤트도 새 포커스로 갈 수 있다. 같은 출처 iframe과 열린 Shadow DOM만 참조로 찾을 수 있으며 교차 출처 iframe과 닫힌 Shadow DOM은 이 기능으로 확장하지 않는다.

2026-10-16 독립 ego Chromium 검사에서 이 경합의 실제 사례를 관찰했다. 문서가 포커스되지 않은 상태에서 준비는 성공했지만 첫 CDP 입력이 문서를 활성화하면서 지연된 focus 핸들러가 다른 textarea로 옮겼고, Enter도 그곳에 전달됐다. 원래 준비 거절을 기대했던 검사는 실패로 보존했다. 이후 같은 문서가 이미 포커스된 조건에서 새 참조로 준비만 검사하자 핸들러가 즉시 실행됐고 `FOCUS_NOT_REACHED`, `attempted:false`, 엔진 요청 0개를 확인했다. 별도 후속 결과로 원래 경합을 해결했다고 판정하지 않는다. 실제 Naru 배경 입력에서도 이 경계를 확인해야 한다.

`nativeVirtualKeyCode`는 0으로 보낸다. 설치된 Chromium 버전의 [Input 처리 구현](https://github.com/chromium/chromium/blob/156.0.8078.17/content/browser/devtools/protocol/input_handler.cc)은 이 경우 macOS 이벤트를 만들지 않고, 처리되지 않은 키를 브라우저 UI로 넘기지 않도록 설정한다. 같은 버전의 [렌더러 포커스 구현](https://github.com/chromium/chromium/blob/156.0.8078.17/content/browser/renderer_host/render_widget_host_impl.cc)은 페이지 포커스를 변경한다. 페이지의 focus 이벤트까지 발생하지 않는다는 뜻은 아니다. Naru는 네이티브 창이나 사용자 공간 선택 명령을 추가하지 않는다.

## 검증 범위

검증 결과와 정리는 [작업 기록](work-log.md)에 남긴다. 모의 DOM은 참조와 포커스 준비를, Rust 단위 검사는 요청과 응답의 수명주기를 검증한다. 별도 ego Chromium의 기본 키 동작 결과는 Naru CEF의 통합 성공으로 해석하지 않는다. ego의 공개 CDP 호출은 대상 탭을 자동 활성화하므로 배경 탭 선택 보존의 증거도 아니다.

`tests/native_press_smoke.py`는 실제 Naru에서 버튼, 줄바꿈, 제출 버튼, 취소, 포커스 거절, Shadow DOM과 iframe, 지원 키 42개 형식의 효과와 focusedRef, 배경 AI 입력 중 사용자 선택 보존을 확인하는 후속 검사다. 앱을 실행하지 않은 구문 검사는 해당 통합 검사의 성공으로 기록하지 않는다. 외부 사이트, 사용자 입력과의 경합, 장시간 사용과 실제 메모리 측정도 별도 검증이 필요하다.
