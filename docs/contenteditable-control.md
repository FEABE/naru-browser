# HTML 편집 영역의 텍스트 입력

2026-10-16 소스는 기존 `fill TAB REF TEXT`를 명시적인 HTML `contenteditable` 영역으로 확장한다. 입력 전체를 일반 텍스트로 바꾸며 새 명령이나 저장소를 추가하지 않는다. 최신 Naru 앱과 CLI 번들의 실행 검증은 별도다.

```sh
lightbrowser read 3
lightbrowser fill 3 '현재 read의 ref' '입력할 텍스트'
lightbrowser task run TASK_ID REV fill 3 '현재 read의 ref' '입력할 텍스트'
```

## 지원 범위

`contenteditable`이 빈 속성, `true`, `plaintext-only`이고 브라우저의 `isContentEditable`도 true인 HTML 요소를 대상으로 한다. Naru는 부모 요소가 편집 가능하지 않은 명시적 영역만 교체한다. 편집 가능한 부모 안의 중첩 영역이나 상속만 받은 자식 ref를 자동으로 부모에 돌려 쓰지 않는다. `contenteditable=false` 경계 안에 새로 선언한 영역은 지원한다. 문서 전체의 `designMode`는 지원하지 않는다. 이는 Naru의 대상 선택 정책이며 HTML 표준의 editing host 정의를 대체하지 않는다.

`read`는 빈 속성과 `plaintext-only`도 찾고, 기존 240자 텍스트 요약과 순회 예산으로 현재 내용을 반환한다. 공백과 줄바꿈을 정규화한 관찰 요약이며 입력 원문이나 실제 화면의 줄 배치를 그대로 복제한 값은 아니다. 열린 Shadow DOM과 접근 가능한 같은 출처 iframe은 기존 참조 문맥을 사용한다.

`fill`은 영역의 자식 내용 전체를 텍스트 노드로 교체한다. HTML처럼 보이는 문자열도 문자 그대로 넣으며 서식, 링크와 이미지, 편집 불가 자식을 보존하지 않는다. 자식이 없고 요청이 빈 문자열이거나, 자식이 정확히 텍스트 노드 하나이고 내용이 같을 때만 포커스와 이벤트 없는 무동작으로 끝낸다. 같은 글자를 가진 복잡한 마크업은 전체 교체 대상이다.

처음에는 다른 값이었지만 focus 핸들러가 먼저 단순 텍스트를 요청값으로 맞춘 경우도 `changed:false`로 반환한다. 이때 포커스는 이미 실행됐으며 Naru의 추가 교체와 입력 이벤트만 생략한다.

## 이벤트와 결과

포커스 뒤 참조 세대, 원문서, 대상의 가용성과 지원 조건, `aria-readonly`를 다시 확인한다. 취소 가능한 합성 `beforeinput`을 먼저 보내고 취소되지 않은 경우에만 텍스트를 교체한다. 그 뒤 합성 `input`을 보내며 두 이벤트의 `inputType`은 `insertText`, `data`는 요청한 문자열이다. 이 경로는 `change`를 만들지 않는다. input과 textarea는 값 설정, input과 change 순서를 사용하는 [별도 경로](native-fill-control.md)이며 후속 소스에서 최종 값 확인을 추가했다.

두 이벤트는 `trusted:false`이며 물리 키 입력이나 사용자 활성화를 보장하지 않는다. 실제 네이티브 편집 명령, Selection과 target ranges, undo 이력, 임의 편집기의 내부 데이터 모델을 재현하지 않는다. 줄바꿈 문자는 저장하지만 표시 방식은 페이지의 CSS에 따른다. 브라우저의 표준 편집 가능 요소를 지원한다는 것이 모든 채팅창과 본문 편집기의 호환성을 뜻하지 않는다.

| 상황 | 결과 |
| --- | --- |
| 단순 텍스트가 이미 같음 | `ok:true`, `changed:false` |
| 교체 후 같은 삽입 텍스트와 대상 확인 | `ok:true`, `changed:true` |
| `beforeinput` 취소 | `INPUT_CANCELED`, `attempted:false` |
| 교체 전 대상이나 참조 변경 | 실행을 거절하고 최신 ref를 요구한다. |
| 교체 후 요소나 문서, 가용성 변경 | `FILL_STATE_UNCONFIRMED`, `attempted:true` |
| 같은 삽입 텍스트를 페이지 핸들러가 바꿈 | `FILL_STATE_NOT_REACHED`, `attempted:true` |

`attempted`는 Naru의 DOM 교체 시도 여부다. false여도 focus나 beforeinput 핸들러가 만든 페이지 부작용이 없었다고 단정하지 않는다. 페이지가 같은 글자를 다른 노드로 다시 만들면 결과를 확정하지 않고 재관찰을 요구한다. 자동 재시도와 페이지 변경 롤백은 없다. 동기 확인 이후 실행되는 페이지 작업은 후속 `read`로 확인한다.

Rust는 원래 요청이 fill이고 응답의 action도 fill일 때만 `attempted`, `trusted`의 boolean 두 개를 오류 상세에 전달한다. 입력 문자열과 임의 페이지 필드는 복사하지 않는다. 이미 발행한 동작 중 제어권이 바뀌면 기존 `outcomeUnknown` 취소 계약을 유지한다.

## 자원과 검증 경계

입력은 기존 소켓 요청 1MiB 제한을 따른다. 현재 값을 비교하기 위해 전체 `textContent`나 `innerHTML`을 복제하거나 새 DOM 순회를 만들지 않는다. 읽기는 기존 노드, 출력과 참조 예산을 공유한다. 요청 문자열, 새 텍스트 노드와 페이지 이벤트는 일시적으로 함께 존재할 수 있으며 이 설계는 Chromium 전체 RAM 상한이 아니다. 새 worker, 타이머, renderer와 파일 저장은 없다.

구현 검증, 독립 Chromium 검사, 실제 Naru CEF 통합과 네이티브 입력을 구분해 [작업 기록](work-log.md)에 남긴다. 외부 편집기, 실제 IME 조합과 장시간 메모리 검증은 별도다.

참고 자료는 [HTML contenteditable 규격](https://html.spec.whatwg.org/multipage/interaction.html#contenteditable), [Input Events Level 2](https://www.w3.org/TR/input-events-2/#interface-InputEvent-Attributes), [MDN의 직접 DOM 변경과 undo 설명](https://developer.mozilla.org/en-US/docs/Web/API/Document/execCommand)이다. 직접 DOM 교체와 합성 이벤트는 Naru의 동작 계약이며 네이티브 편집의 동일한 재현이라고 설명하지 않는다.
