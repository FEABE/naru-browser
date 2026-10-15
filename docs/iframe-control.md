# 접근 가능한 iframe의 CLI 관찰과 입력

2026-10-15 소스 구현이다. 기존 앱 번들에는 반영하지 않았고 실제 CEF 실행 검증은 남아 있다. 사용자 선택으로 탭 복원과 세션 JSON은 보류했으며 이번 기능은 저장소를 추가하지 않는다.

## 범위와 결과

기존 `read`, `click`, `fill`, `select`, `press Enter` 호출 형식을 유지하면서 메인 문서에서 접근 가능한 iframe 문서와 그 안의 열린 Shadow DOM을 읽는다. `interactive`는 기존 평면 배열이며 프레임 내부 항목에만 `frame` ID를 추가한다. 기존 native 탐색 세대 접두어와 task revision 검사는 그대로 적용된다.

`page.frames`는 해당 read에서만 유효한 `id`, `parent`, `label`, `status`를 반환한다. 부모는 `main` 또는 다른 프레임 ID다. 접근에 성공한 문서만 실제 `url`과 `title`을 제한 길이로 추가한다. status는 `accessible`, `unavailable`, `limit`다. inaccessible이나 아직 준비되지 않은 문서를 비어 있는 본문으로 추정하지 않는다. 선언된 src나 URL 문자열 비교로 실제 접근 권한을 판정하지 않고 `contentDocument`의 현재 결과를 사용한다. 다른 출처, sandbox의 opaque origin, RemoteFrame 등은 null이나 접근 예외가 될 수 있으므로 모두 cross-origin이라고 단정하지 않는다.

상한은 프레임 메타데이터 16개, 프레임 중첩 8단계다. unavailable과 깊이 limit 항목도 16개에 포함한다. 프레임 상한은 contentDocument 조회 전에 확인한다. 기존 본문 12,000자, 요소 120개, 노드 방문 10,000개, 열린 shadow root 64개, 순회 깊이 64와 옵션 전체 240개 예산은 문서마다 새로 부여하지 않고 전체 read가 공유한다. 잘림은 `truncated.frames`, `truncated.frameDepth`와 기존 필드로 구분한다. 라벨 스캔은 iframe의 본문으로 넘어가지 않는다.

## 참조 수명과 입력

프레임 참조는 요소와 소속 문서, 부모 iframe부터 메인 문서까지의 연결에 묶인다. action 전에 각 iframe이 원래 부모 문서에 연결돼 있고 현재 contentDocument가 기록한 문서와 같은지 확인한다. 새 탐색, 제거, 다른 문서로 이동한 요소와 접근 불가 전환은 오래된 참조로 거절한다. iframe과 상위 요소의 숨김, inert, aria-hidden, 비활성 상태 및 iframe의 0 크기도 검사한다. focus나 keydown 처리에서 문서가 바뀌면 후속 값 쓰기, 제출이나 keyup을 이전 문서에 계속 전달하지 않는다. input이나 keydown을 이미 전달한 뒤 대상이 바뀌면 부분 동작을 detail에 명시하고 change, 제출과 keyup 중 남은 동작은 실행하지 않는다.

스타일, Range, native form setter와 Event 생성자는 요소가 속한 문서의 realm을 사용한다. 입력은 합성 DOM 이벤트이며 trusted 장치 입력과 같지 않다. 클릭이나 입력 자체가 페이지 탐색을 시작할 수 있고 이벤트를 전달한 뒤 페이지가 값을 바꿀 수 있으므로 실제 효과는 새 read로 확인해야 한다. 입력 결과를 무조건 성공한 업무 처리로 설명하지 않는다.

viewport와 scroll은 메인 창 기준을 유지한다. iframe 내부 스크롤 명령, 픽셀 좌표 변환, native 좌표 클릭, 요소와 screenshot의 정합, 교차 출처 문서 내용이나 닫힌 Shadow DOM, 캔버스는 지원 범위가 아니다. 기존 CEF `grantUniveralAccess:false`를 유지한다.

## 엔진 근거와 자원

설치 Chromium 156.0.8078.17 소스에서 DOMWindow::Wrap은 현재 ScriptState의 World로 자식 WindowProxy를 선택하고 contentDocument 바인딩도 같은 World ID로 자식 문서를 감싼다. 프레임별 context와 realm은 별도다. [DOMWindow](https://github.com/chromium/chromium/blob/156.0.8078.17/third_party/blink/renderer/core/frame/dom_window.cc#L248), [문서 바인딩](https://github.com/chromium/chromium/blob/156.0.8078.17/third_party/blink/renderer/bindings/scripts/bind_gen/interface.py#L1667), [context 생성](https://github.com/chromium/chromium/blob/156.0.8078.17/third_party/blink/renderer/bindings/core/v8/local_window_proxy.cc#L321).

contentDocument의 반환값은 DOM 보안 출처 검사를 거치며 접근할 수 없으면 null이다. same security origin이어도 RemoteFrame인 경우 접근할 수 없다. [보안 검사](https://github.com/chromium/chromium/blob/156.0.8078.17/third_party/blink/renderer/bindings/core/v8/binding_security.cc#L79), [null 반환 바인딩](https://github.com/chromium/chromium/blob/156.0.8078.17/third_party/blink/renderer/bindings/scripts/bind_gen/interface.py#L995).

자식 WindowProxy 접근은 해당 isolated context를 지연 생성할 수 있다. 프레임 수와 깊이 제한은 한 read가 접근하는 문서와 응답을 제한한다. 반복 read에서 다른 프레임을 방문하면 추가 context가 생성될 수 있으므로 브라우저 전체 context가 16개라는 뜻은 아니며 페이지 전체 renderer RAM 상한도 아니다. [WindowProxyManager](https://github.com/chromium/chromium/blob/156.0.8078.17/third_party/blink/renderer/bindings/core/v8/window_proxy_manager.h#L51). refs와 문서 연결은 다음 read에서 교체하고 탭 종료나 메인 문서 교체 때 해당 realm과 함께 해제된다. 새 renderer, 반복 타이머, worker와 파일 저장은 추가하지 않는다. 실제 RAM 전후 비교는 미실행이다.

## 검증 경계

모의 DOM은 문서 identity, 부모 연결, 행동 전후 검증과 예산을 검사한다. 실제 same-origin policy, 여러 realm의 native setter와 이벤트 전달, layout과 CSS transform, frame 탐색 및 renderer 경쟁을 재현하는 것은 아니다. `tests/shadow_smoke.py`에는 접근 가능한 프레임과 중첩 Shadow DOM, 폼 입력과 선택, Enter, 삭제 후 stale ref 및 opaque sandbox 제외를 확인할 fixture와 검사를 추가했다. 앱에 접속하는 이 검사는 아직 실행하지 않았다. 실제 앱이나 키체인 요청을 자동으로 시작하지 않는다. 최신 결과는 [작업 기록](work-log.md)을 기준으로 한다.
