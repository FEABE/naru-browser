# 일반 입력칸의 값 확인

`fill TAB REF TEXT`는 input과 textarea의 네이티브 value setter를 사용하고 합성 input, change 이벤트를 전달한다. 2026-10-16 후속 소스는 이 순서를 유지하면서 최종 값이 요청한 문자열과 같은지 확인한다. [contenteditable 입력](contenteditable-control.md)은 별도의 전체 텍스트 교체 경로다.

## 성공과 실패의 의미

숫자, 날짜 등 입력 종류에 따라 브라우저가 문자열을 정규화하거나 비울 수 있다. 페이지의 input이나 change 핸들러가 값을 다시 바꿀 수도 있다. 이벤트를 보냈다는 사실만으로 요청값이 남았다고 판단하지 않는다.

- 지원하는 대상인지 확인한 뒤 포커스를 준다. 포커스 처리 뒤에도 현재 read의 참조, 원문서와 프레임, 연결, 가시성, 활성 상태와 읽기 전용 여부를 확인한다.
- 포커스 뒤 지원하는 입력 종류를 고정하고 그 문서의 네이티브 setter로 값을 넣는다. input과 change 사이에도 대상과 종류를 확인한다. 페이지가 대상을 교체하거나 종류를 바꾸면 남은 이벤트를 보내지 않는다.
- 마지막에는 같은 문서의 input 또는 textarea 프로토타입에 정의된 value getter로 실제 값을 읽는다. 요소에 덧씌운 value getter만으로 성공을 위장할 수 없다.
- 같은 대상에 요청한 문자열이 남아 있을 때만 성공한다. 값이 다르면 `FILL_STATE_NOT_REACHED`, 대상이나 문서, 편집 조건이 바뀌어 결과를 확정할 수 없으면 `FILL_STATE_UNCONFIRMED`다.

비교는 요청 문자열 그대로다. 예를 들어 잘못된 숫자가 빈 문자열로 바뀌거나 textarea의 CRLF가 LF로 정규화되면 값 미도달 오류다. Naru가 각 입력 종류의 정규화 규칙을 다시 구현하거나 결과를 원하는 값으로 간주하지 않는다. 필요한 형식으로 요청을 준비하고 오류 뒤에는 현재 페이지를 다시 읽는다. 자동 재시도나 이전 값 복구는 하지 않는다.

성공은 동기 이벤트가 끝난 시점의 value 확인이다. HTML validity, 서버 저장, 폼 제출, 프레임워크의 내부 상태와 이후 비동기 변경까지 확인한 결과는 아니다. 입력값이 이미 같아도 기존과 같이 input과 change를 전달한다. 일반 입력칸 경로에 새로운 beforeinput 이벤트나 이벤트 없는 무동작을 추가하지 않는다.

## 오류 상세와 자원

오류의 `attempted`는 Naru가 네이티브 value setter 호출을 시도했는지를 나타낸다. false여도 focus 핸들러가 일으킨 부작용이 없다는 의미는 아니다. `trusted:false`는 합성 이벤트 경로임을 뜻한다. 요청값, 실제 값과 페이지 예외 원문은 오류 상세에 넣지 않으며 비밀번호 입력도 같은 원칙을 따른다.

기존 Rust 오류 전달 경로는 원래 요청과 응답 action이 fill일 때 boolean 두 개만 전달한다. 작업 제어권이나 문서 세대가 바뀐 뒤의 응답 취소와 `outcomeUnknown` 처리는 유지한다.

새 명령, worker, 타이머, renderer, 저장 파일과 참조 캐시는 없다. 확인 중 실제 value 문자열 하나를 읽는다. 이미 웹페이지가 보유한 값의 길이를 제한하거나 브라우저 전체 메모리를 줄이는 기능은 아니다.

## 검증 경계

기존 소스의 독립 ego lite Chromium 152 검사에서 정규화, 핸들러의 값 되돌림과 대상 제거 뒤에도 성공을 반환하는 사례를 재현했다. 같은 브라우저의 자체 페이지와 격리된 실행 환경에서 최종 소스를 확인하며 구체적인 결과는 [작업 기록](work-log.md)에 남긴다. 이 검사는 Naru의 소켓과 실제 CEF 연결, macOS 입력이나 외부 서비스 호환성 검증을 대신하지 않는다.

규격 근거는 [HTML input의 종류별 값 처리](https://html.spec.whatwg.org/multipage/input.html)와 [textarea의 API value](https://html.spec.whatwg.org/multipage/form-elements.html#concept-textarea-api-value)다. 설치된 Chromium의 특정 변환 결과와 최신 규격의 설명은 구분한다.
