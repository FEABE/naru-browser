# 체크박스와 라디오 제어

2026-10-15 소스에 `check TAB REF true|false`를 추가했다. 앱과 CLI의 기존 배포 번들은 교체하지 않았다. 독립 Naru의 CLI 폼 제어를 보완하는 기능이며 Preseller 내장은 별도 단계다.

```sh
lightbrowser read 3
lightbrowser check 3 '현재 read의 ref' true
lightbrowser task run TASK_ID REV check 3 '현재 read의 ref' false
```

CLI 값은 소문자 `true`, `false`만 허용한다. 원본 JSON 요청의 `checked`는 boolean이어야 한다. 탭 소유권, 작업 revision, 탐색 epoch와 읽기 세대를 기존 동작과 같이 검사한다. 작업 소유 탭에는 해당 작업의 `task run`을 사용한다.

## 실행 계약

네이티브 HTML 체크박스의 선택과 해제, 라디오의 선택만 지원한다. 라디오에 `false`를 보내거나 `indeterminate`가 true인 체크박스를 요청하면 `UNSUPPORTED_STATE`를 반환한다. 라디오는 다른 항목을 `true`로 선택한다. ARIA 역할만 가진 사용자 정의 컨트롤은 `UNSUPPORTED_ELEMENT`다. 혼합 상태를 임의로 해제하거나 속성만 덮어쓰지 않는다.

`read`는 체크박스에 `checked`, `indeterminate`를 반환하고 라디오에는 `checked`를 반환한다. 현재 상태가 요청과 같으면 포커스, 클릭, 이벤트 없이 성공하며 `changed: false`다. 다른 상태라면 focus 뒤 요소의 연결, 원문서, 읽기 세대, 가용성과 입력 타입을 다시 확인한다. focus 핸들러가 이미 원하는 상태로 만들었다면 클릭하지 않고 `changed: false`를 반환한다. 이때 페이지 자체의 focus 부작용이 없었다는 뜻은 아니다.

변경이 필요하면 DOM `click()`을 한 번 호출한다. 브라우저의 기본 체크박스, 라디오 활성화가 상태와 후속 이벤트를 처리하게 두며 `checked` setter나 수동 `input`, `change` 이벤트를 사용하지 않는다. 클릭 직후 같은 요소의 상태를 동기적으로 확인한다. 자동 재시도는 없다. 이 동작은 사용자 입력의 `isTrusted`나 사용자 활성화를 보장하지 않는다. 응답의 `trusted: false`는 실행한 DOM 클릭을 설명하며 브라우저가 생성하는 후속 `input`, `change`의 신뢰값까지 false라는 뜻이 아니다.

| 결과 | CLI 응답 | 다음 동작 |
| --- | --- | --- |
| 원하는 상태 확인 | `ok: true`, `result.page.changed`, `checked` | 페이지의 후속 처리가 필요하면 다시 읽는다. |
| 클릭 취소, 핸들러의 되돌림이나 혼합 상태 | `CHECK_STATE_NOT_REACHED`, `error.details.attempted: true`, `checked`, 해당 체크박스의 `indeterminate` | 현재 페이지를 관찰하고 원인을 판단한다. |
| 클릭 후 요소 제거, 타입 변경, 숨김이나 비활성 | `CHECK_STATE_UNCONFIRMED`, `error.details.attempted: true` | 성공이나 미실행으로 단정하지 않는다. 현재 상태 필드는 생략한다. |
| 클릭 호출이나 이후 검사 중 예외 | `ACTION_FAILED`, `error.details.attempted: true` | 예외 원문은 노출하지 않으며 다시 읽어 판단한다. |
| 클릭 전 참조나 제어권 만료 | 기존 `STALE_REF`, 작업 오류 | 최신 작업 상태와 ref를 확인한다. |

실패 상세는 원래 요청이 `check`인 경우에만 `attempted`, `trusted`, `checked`, `indeterminate`의 boolean을 복사한다. 페이지가 덧붙인 문자열과 임의 필드는 전달하지 않는다. 이미 발행한 동작 중 제어권이 끊기면 기존 `outcomeUnknown` 취소 계약을 따른다. `attempted`가 없다고 해서 모든 페이지 부작용이 없었다고 추정하지 않는다.

성공은 관찰 시점의 상태다. 비동기 핸들러, 이후 탐색이나 나중에 실행되는 focus 핸들러의 결과를 고정하지 않는다. 배경 탭에서 focus 이벤트가 지연될 수 있으며 검사를 위해 사용자 공간을 자동 전환하지 않는다. 열린 Shadow DOM과 접근 가능한 같은 출처 iframe은 기존 ref 문맥을 재사용한다. 교차 출처 iframe, 닫힌 Shadow DOM, trusted native 입력은 이번 기능에 포함하지 않는다.

## 근거와 검증 범위

[HTML 입력 요소 규격](https://html.spec.whatwg.org/multipage/input.html#the-input-element)은 클릭 전 선택 변화와 클릭 취소 시 복원을 정의한다. [체크박스 활성화](https://html.spec.whatwg.org/multipage/input.html#checkbox-state-(type=checkbox))는 분리된 요소에서 후속 입력 이벤트 생성을 중단한다. 직접 속성을 설정해 이 기본 동작을 우회하지 않도록 구현했다.

모의 DOM은 70개를 통과했다. 기존 55개와 새 15개로, 호출 횟수, 무동작, 취소, focus 중 변경, 클릭 후 변경, 세대 만료, Shadow DOM과 iframe 참조를 포함한다. 최초 68개 통과와 2개 실패는 동작 도중 새 read가 생겼을 때 captured ref의 세대를 확인하지 않는 누락을 발견했다. 공통 ref 검사를 보완하고 통과했다. 모형은 브라우저 전체를 재현하지 않는다.

설치된 ego lite의 검사 공간 4, 자체 로컬 HTTP 페이지에 현재 `src/dom_bridge.js`를 격리된 JavaScript 환경으로 주입해 24개 시나리오를 통과했다. 클릭 취소 시 복원, 클릭 한 번과 중복 무동작, 핸들러가 만든 상태, Shadow DOM과 같은 출처 iframe을 확인했다. 최초 실행은 분리된 input에서도 `input`, `change`가 발생할 것이라는 검사 가정에서 실패했다. 실제 상태와 HTML 규격을 확인해 검사 모형을 바로잡았고 제품 클릭 코드는 바꾸지 않았다.

최종 24개 검사는 해당 페이지에 CDP 포커스 에뮬레이션을 사용했다. 페이지 핸들러는 실제 Chromium에서 실행됐지만 macOS 창 포커스나 실제 장치 입력을 증명하지 않는다. `artifacts/check-control-v1-ego-result.json`에 bridge 해시, 결과, 이벤트와 포커스 조건을 보존했다. 에뮬레이션을 해제하고 검사 공간을 종료했다. 이 결과는 Naru의 소켓, Rust 요청 전달, CEF 콜백이나 최신 앱 통합 검사를 대신하지 않는다.

`tests/check_smoke.py`는 현재 소스를 포함한 Naru 앱과 CLI에서 실행할 로컬 통합 검사다. 자체 작업과 탭만 만들며 초기 사용자 탭과 선택, 다른 작업의 상태를 확인하고 정리한다. 문법만 검사했고 실행하지 않았다. Rust 검증 결과와 빌드 정리는 [작업 기록](work-log.md)을 따른다.

## 자원

기존 refs와 제한된 읽기 응답에 boolean과 세대 값을 추가한다. 라디오 그룹을 별도 수집하거나 전체 DOM을 다시 탐색하지 않는다. 오류 상세의 추가 필드는 최대 네 boolean이다. 새 worker, 타이머, renderer, 프로세스나 저장 파일은 없다. 실제 앱 RAM과 장시간 자원 회수는 미측정이며 작은 구현 변경을 메모리 절감으로 해석하지 않는다.
