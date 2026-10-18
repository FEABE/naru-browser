# 현재 선택 영역에 텍스트 넣기

`lightbrowser task run ID REV insert TAB REF TEXT`는 최근 read의 참조가 가리키는 입력칸이나 HTML 편집 영역의 현재 커서와 선택 영역에 Chromium 텍스트 입력을 요청한다. 전체 내용을 바꾸려면 기존 fill을 사용한다. insert는 문자별 keydown을 만들거나 클립보드에 접근하지 않는다.

## 준비와 실제 입력

text, search, tel, url, password 입력칸과 textarea, 명시적 contenteditable host를 지원한다. 입력칸은 native value와 selectionStart, selectionEnd, selectionDirection을 읽고 포커스 전후 같은 값과 선택을 유지하는지 확인한다. setSelectionRange, select, Range 변경으로 선택을 덮어쓰지 않는다. contenteditable은 같은 지원 host 안에 있는 단일 Range가 필요하다. 선택이 없거나 다른 편집 영역과 비편집 영역을 가로지르면 준비를 거절한다. 이미 선택한 대상에 커서를 임의로 새로 만들지 않는다.

키 명령으로 커서를 옮기거나 일부를 선택한 뒤 새 read의 같은 요소 참조로 insert를 호출할 수 있다. read의 focusedRef는 관찰 가능한 범위에서만 제공되므로 null이면 본문의 적절한 요소 참조를 확인한다. 열린 Shadow DOM과 접근 가능한 같은 출처 iframe의 native accessor와 Selection을 해당 문서에서 사용한다. cross-origin iframe과 닫힌 Shadow DOM은 여전히 미지원이다.

준비가 끝나면 기존 입력 배치의 Input.insertText 한 번을 사용한다. 원래 탭과 Browser, 작업 제어권, 문서와 isolated context를 유지한 채 사후 결과를 확인한다. 준비부터 사후 확인까지 같은 탭의 진행 중 요청을 유지한다. 값 직접 쓰기, 합성 input 이벤트, 빈 입력을 Backspace로 바꾸는 fallback과 자동 재시도는 없다.

## 결과의 의미

성공 응답의 공통 page 필드는 다음과 같다. 일반 입력칸 예시다.

```json
{
  "ok": true,
  "action": "insert",
  "input": "chromium",
  "attempted": true,
  "dispatchAcknowledged": true,
  "verification": "value"
}
```

입력칸과 textarea의 `verification:"value"`는 준비 때의 값과 선택 범위로 만든 예상 결과가 사후 native 값과 정확히 같다는 뜻이다. 원래 값, 비밀번호와 선택한 본문은 응답에 넣지 않는다. 기존 선택에 같은 문자를 넣거나 빈 문자열을 삽입해 값이 그대로인 경우도 상태 일치만 확인한다. 해당 이벤트가 실제 발생했거나 서버가 내용을 저장했다는 뜻은 아니다. maxlength, 줄바꿈 정규화, 취소나 페이지의 값 되돌림 때문에 달라지면 INSERT_STATE_NOT_REACHED다.

contenteditable의 `verification:"input-event"`는 원래 host를 대상으로 trusted input을 관찰했고 inputType이 insertText이며 data가 요청한 텍스트와 같았다는 뜻이다. rich DOM의 Range offset과 innerText 줄바꿈을 동일한 좌표로 보지 않는다. 문서 전체의 예상 문자열, 서식이나 외부 편집기 내부 모델, undo를 검증했다고 반환하지 않는다. 페이지가 이벤트 뒤 내용을 다시 바꿀 수 있으므로 새 read와 필요한 화면 확인을 이어간다. 닫힌 Shadow DOM 경계의 이벤트 retargeting과 동시에 사람이 넣은 동일 입력을 완전히 구분하지 못한다.

CE의 일치하는 입력 이벤트를 얻지 못하거나 원래 대상과 문서, 종류를 확인하지 못하면 INSERT_STATE_UNCONFIRMED다. 다른 대상의 입력을 관찰하면 INPUT_TARGET_MISMATCH다. 입력이 이미 전달됐을 수 있으므로 오류의 attempted와 outcomeUnknown을 보존한다. beforeinput 취소가 반드시 별도의 취소 코드로 구분되는 것은 아니다. 준비에서 값과 선택이 바뀌면 SELECTION_NOT_REACHED 등 attempted:false 오류이며, focus 핸들러 자체의 부작용이 없다는 뜻은 아니다.

빈 문자열도 동일한 엔진 경로로 요청한다. 입력칸은 선택 제거 후 기대 값으로 판단하지만 CE의 빈 입력은 다른 inputType이나 data, 이벤트 미발생 때문에 결과가 unconfirmed일 수 있다. 엔진의 실제 삭제 효과는 E2E에서 확인하며 성공한 것으로 추정하지 않는다.

## 수명과 메모리

원래 참조와 읽기 세대, 종류를 묶은 임시 관찰은 bridge마다 하나다. 새로운 읽기와 다른 동작, 결과 소비 또는 5초 단발 만료에서 정리한다. renderer가 멈추면 타이머 실행도 늦어진다. 입력칸의 예상 문자열은 최대 1Mi UTF-16이며 원래 값과 삽입 길이를 합산해 검사한다. contenteditable은 기존 노드 10,000개, 깊이 64와 텍스트 상한을 적용한다. native getter가 원래 문자열을 읽는 순간까지 완전한 할당 상한을 보장하는 것은 아니다.

관찰자는 Event 객체와 페이지 원문을 로그에 남기지 않고 입력 대상과 데이터 일치 여부를 제한된 상태로 기록한다. Rust는 원래 요청 텍스트, 작은 mode 값과 관찰 token을 기존 입력 상태에 추가한다. 새 프로세스, UI renderer, 반복 polling, 디스크 저장이나 페이지 전체 사본은 없다. 실제 메모리 감소는 측정하지 않았다.

## 남은 경합과 E2E

후속 소스는 [임시 포커스 준비](focus-input.md)의 exact-session enable ACK 뒤 원 참조의 선택을 새로 확인한다. 입력 한 번의 ACK 뒤 disable ACK를 기다린 다음 기존 선택·값 또는 입력 이벤트를 읽기 전용으로 검증하며 다시 focus하지 않는다. 해제 때문에 발생한 페이지 변경도 결과 확인에 포함한다.

준비와 Chromium 입력은 원자적이지 않다. 임시 설정은 첫 입력 때의 페이지 포커스 전환을 준비 앞으로 옮기는 보완이며 모든 timer·rAF·사람 입력이나 OOPIF의 지연 이벤트를 소진하는 장치가 아니다. 이후 포커스 변경과 이미 발생한 잘못된 입력을 예방·복구했다고 설명하지 않는다. 실제 최신 Naru에서 효과와 사용자 화면 보존을 확인해야 한다.

[통합 E2E 목록](ego-parity-e2e.md)의 06과 07에서 커서 이어쓰기, 키로 선택 후 치환, textarea 여러 줄, plain과 rich contenteditable, 접근 가능한 iframe과 Shadow DOM, 취소와 maxlength, 대상 변경, 빈 값, 비밀번호 비노출과 사용자 공간 보존을 확인한다. `tests/insert_text_smoke.py`와 자체 fixture는 그 준비이며 실제 실행 전에는 통과로 표시하지 않는다. 구문 검사, release 빌드와 실제 Naru CEF 검증은 [작업 기록](work-log.md)에 나누어 기록한다.
