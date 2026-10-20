# 참조로 목록과 프레임 문서 스크롤하기

## 명령

`scroll TAB REF DX DY`는 최근 읽기에서 반환한 목록이나 문서 참조를 상대 이동한다. `Command::ScrollElement`로 전달하며 기존 `scroll TAB DX DY`의 메인 페이지 스크롤 형식은 유지한다.

`TAB`은 양의 32비트 정수, `REF`는 기존 참조 입력과 같은 1~256바이트 문자열이다. `DX`, `DY`는 각각 -10,000~10,000 CSS 픽셀 정수다. 숫자를 문자열로 보내거나 소수, 범위 밖 값을 보내면 요청을 거절한다. 참조 형식의 구체적인 유효성은 원래 문서와 스냅샷을 확인하는 실행 경로에서 검사한다.

```bash
lightbrowser read 3
lightbrowser scroll 3 '읽기 결과에서 고른 scrollable ref' 0 600

lightbrowser read-frame 3 'frames 결과에서 고른 FRAME'
lightbrowser scroll 3 '해당 프레임 문서의 scrollable ref' 0 600

lightbrowser task run TASK_ID REV read-page 3
lightbrowser task run TASK_ID REV scroll 3 '문서별 결과에서 고른 scrollable ref' -200 0
```

예시의 탭 번호, 참조, 프레임 ID, 작업 ID와 revision은 실제 응답으로 바꿔 사용한다. 읽기 결과에 `scroll` 메타데이터가 있는 항목의 `ref`를 그대로 전달한다. 새 읽기가 기존 참조를 만료시킬 수 있으므로 마지막 읽기 결과에서 고른다.

로컬 소켓의 명령 형식은 다음과 같다. 작업 범위에서는 이 객체를 기존 `task_run.command`에 넣는다.

```json
{
  "type": "scroll_element",
  "tab": 3,
  "ref": "읽기 결과의 실제 참조",
  "x": 0,
  "y": 600
}
```

## 읽기 메타데이터

`read`, `read-frame`, `read-page`는 탐색 범위에서 찾은 스크롤 대상에 다음 형태의 `scroll` 정보를 붙인다. 아래 수치는 필드 설명용 예시다.

```json
{
  "target": "element",
  "axes": { "x": false, "y": true },
  "position": { "x": 0, "y": 120 },
  "extent": { "width": 320, "height": 2400 },
  "viewport": { "width": 320, "height": 480 }
}
```

| 필드 | 의미 |
| --- | --- |
| `target` | `element`는 개별 목록, `document`는 해당 문서의 스크롤 대상 |
| `axes` | 읽은 시점에 내용 크기와 overflow 설정으로 확인한 축별 지원 여부 |
| `position` | 읽은 시점의 스크롤 좌표 |
| `extent` | 스크롤 가능한 내용 전체의 너비와 높이 |
| `viewport` | 해당 스크롤 대상의 내부 표시 영역 크기 |

요소는 `overflow-x` 또는 `overflow-y`가 `auto`, `scroll`이고 해당 축의 내용이 표시 영역보다 큰 경우에 대상이 된다. 메인 창만이 아니라 선택한 요소나 프레임 문서의 좌표를 사용한다. 화면 캡처에서 가려짐까지 확인한 결과는 아니다. 축 지원 여부는 읽기 시점과 실행 시점에 달라질 수 있다.

문서는 `body` 참조로 대표하며 실제 스크롤 요소는 `document.scrollingElement`로 고정한다. `body`가 독립적인 overflow 목록인 경우 이를 별도의 요소 대상으로 동시에 표현하지 않는다. `body`가 없거나 표시 상태, 읽기 예산 때문에 결과에 포함되지 않으면 문서 참조도 제공되지 않을 수 있다.

위치는 0 이상의 값으로 강제 보정하지 않는다. RTL 가로 목록 등에서는 음수 좌표가 정상일 수 있다. `DX`, `DY`도 논리적인 시작과 끝이 아니라 DOM의 x, y 상대 이동량이다.

## 실행과 참조 수명

실행 직전에 참조의 요소, 문서, 스냅샷 세대와 저장된 스크롤 요소를 다시 확인한다. 읽을 때 목록이었던 요소를 실행할 때 문서로 바꾸거나 새 `scrollingElement`로 대체하지 않는다. 숨김, inert, disabled 상태이거나 원래 요소가 사라졌다면 조작을 거절한다.

대상이 유효하면 해당 요소 또는 문서 창에 `scrollBy({left, top, behavior:"instant"})`를 한 번 호출한다. Naru가 대상에 포커스를 주거나 `scrollIntoView`를 호출하지 않으며, 끝에 닿았다고 조상 목록이나 부모 프레임으로 이동을 넘기지 않는다. 별도의 휠 입력이나 클릭도 보내지 않는다. 페이지의 스크롤 처리 코드가 만드는 후속 효과까지 막는 기능은 아니다.

`read-frame`, `read-page`의 프레임 참조는 원래 frame, session, unique context와 `documentId`, generation에 묶인 `frameAct` 경로를 사용한다. 부모 연결과 작업 제어권 검사를 유지하며 현재 선택한 프레임이나 새 컨텍스트로 바꿔 실행하지 않는다. 교차 출처 프레임의 본문을 메인 문서 참조로 우회하지 않는다. 관련 수명은 [프레임 입력](frame-input.md), [여러 프레임 읽기](page-read.md), [DevTools 연결](devtools-routing.md)을 따른다.

작업의 handoff, resume, 탭 이동이나 문서 변경으로 원래 소유권과 revision이 달라지면 이전 참조를 그대로 재실행하지 않는다. 새 작업 상태와 페이지를 읽고 현재 소유자가 후속 조작을 결정한다. 이 명령이 사용자 공간이나 선택 탭을 바꾸는 동작은 추가하지 않는다.

## 응답과 오류

성공 응답의 동작 정보에는 `action:"scroll"`, `input:"dom"`, `target`, `before`, `after`, `changed`, `attempted`가 포함된다. `before`, `after`는 호출 직전과 직후에 동기적으로 읽은 x, y 좌표다. `changed`는 두 좌표 중 하나라도 달라졌는지만 나타낸다.

`changed:false`는 끝에 닿았거나 이동량이 0인 경우에도 가능하다. `changed:true`도 가상 목록의 새 데이터 로딩이나 비동기 렌더링 완료를 뜻하지 않는다. DOM 스크롤 호출과 좌표 관찰이며 물리적인 휠 입력, 네이티브 휠 이벤트, 사용자 활성화의 증거로 해석하지 않는다. 후속 내용을 쓰려면 다시 읽어 확인한다.

| 오류 | 처리 |
| --- | --- |
| `INVALID_REQUEST`, `INVALID_SCROLL` | 참조 길이, 탭 번호, 이동량 형식을 확인한다. |
| `STALE_REF`, `STALE_FRAME_SNAPSHOT`, `FRAME_CHANGED` | 원래 참조나 문서 연결이 만료됐다. 새 읽기 없이 반복하지 않는다. |
| `ELEMENT_NOT_SCROLLABLE` | 현재 대상이 지원되는 목록이나 문서가 아니다. 새 읽기의 `scroll` 항목을 확인한다. |
| `UNSUPPORTED_SCROLL_AXIS` | 0이 아닌 이동을 요청한 축이 지원되지 않는다. 부모로 자동 전환하지 않는다. |
| `ELEMENT_UNAVAILABLE` | 요소의 표시 상태나 실제 스크롤 대상이 변경됐다. 이미 이동했을 가능성을 함께 확인한다. |
| `ACTION_FAILED` | DOM 호출 또는 직후 관찰에서 예외가 발생했다. 같은 이동을 자동 반복하지 않는다. |
| `TASK_STALE_REVISION`, `TASK_PAUSED` | 현재 작업 상태를 확인하고 유효한 제어권 없이 재시도하지 않는다. |

`attempted:true`나 `outcomeUnknown:true`가 있으면 이미 스크롤이 발생했을 수 있다. 오류가 반환됐다는 이유로 같은 상대 이동을 자동 반복하면 두 번 이동할 수 있다. 오류 코드는 CLI 응답의 `error`와 동작 상세에서 확인한다.

## 메모리와 검증 경계

스크롤 항목도 기존 읽기 예산 안에 포함한다. 기본 읽기는 참조 최대 120개, 노드 방문 최대 10,000회를 사용하며 스크롤 대상을 위해 별도의 무제한 DOM 순회를 추가하지 않는다. `read-page`는 문서마다 이 상한을 새로 부여하지 않고 기존 공통 예산을 나눈다.

추가 스크롤 요소 연결은 `WeakRef`로 보관하고 프레임 스냅샷의 기존 요소 `WeakRef` 수명도 유지한다. 제거된 DOM이나 이전 문서를 살려 두기 위한 강한 참조를 추가하지 않는다. 기존 `read`의 제한된 참조 보관 방식 전체를 WeakRef로 교체하는 변경은 아니다. 별도 renderer, worker, 저장소를 만들지 않으며 실제 메모리 절감량은 측정 결과가 있어야 판단한다.

이 문서는 소스 계약을 설명한다. 검사, 빌드와 실제 앱 E2E 결과는 [작업 기록](work-log.md)과 [통합 검증 목록](ego-parity-e2e.md)에서 별도로 확인한다. 문서에 적힌 예시나 계획을 실행 성공으로 계산하지 않는다.
