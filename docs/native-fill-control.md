# 일반 입력칸의 Chromium 입력과 값 확인

`fill TAB REF TEXT`는 입력 전체를 요청 문자열로 교체한다. 후속 소스는 input의 text, search, tel, url, password와 textarea를 Chromium 입력으로 연결한다. email, number, 날짜, 색상 등 나머지 기존 지원 유형은 DOM 값 설정을 유지한다. [contenteditable 입력](contenteditable-control.md)도 후속 소스에서 전체 선택과 Chromium 입력, 렌더된 텍스트 확인 경로로 연결한다. CLI는 읽기 전용 분류에서 확인한 입력 유형과 경로를 고정한다. focus 핸들러가 유형이나 경로를 바꾸면 거절하며 Chromium 입력 실패 후 DOM 방식으로 전환하지 않는다. 새 소스와 실제 Naru 앱에서 검증한 범위는 작업 기록에서 구분한다.

## Chromium 입력 순서

후속 구현은 먼저 읽기 전용으로 입력 종류와 경로를 분류한다. Chromium 경로만 [임시 포커스 준비](focus-input.md)를 사용하며 정확한 session의 enable ACK 뒤 아래 준비를 새로 수행한다. 분류 후 종류나 경로가 바뀌면 다시 분류하거나 DOM 경로로 바꾸어 쓰지 않는다. legacy DOM 경로는 이 임시 설정을 적용하지 않는다.

1. 최근 read의 참조, 원문서와 프레임, 연결, 표시와 활성 상태, 읽기 전용 여부를 확인한다.
2. 대상을 focus한 뒤 같은 조건과 실제 포커스를 다시 확인한다. 열린 Shadow DOM과 같은 출처 iframe의 포커스 연결도 검사한다.
3. 문서의 네이티브 선택 메서드로 전체 값을 선택하고, selectionStart가 0이고 selectionEnd가 현재 값 길이와 같은지 확인한다. 준비 중 값, 입력 종류, 문서나 포커스가 바뀌면 전송하지 않는다.
4. Rust가 원래 요청 문자열로 `Input.insertText`를 한 번 보낸다. 빈 문자열도 같은 요청으로 선택한 내용을 지운다. 이미 같은 값이어도 별도 무동작 처리 없이 Chromium에 맡긴다.
5. 엔진의 접수 응답 뒤 임시 포커스 설정의 disable ACK를 기다린다. 그 뒤 원래 isolated context와 ref에서 같은 입력 종류, 편집 조건과 포커스, 네이티브 value를 읽기 전용으로 확인한다. 새 read나 새 참조로 대상을 바꾸거나 다시 focus하지 않는다.

준비 결과에는 입력값을 담지 않는다. 원 Browser와 문서 세대, 소유권과 요청 활성 상태를 각 단계에서 확인하고, 준비부터 결과 확인까지 같은 탭의 진행 요청을 유지한다. 실패나 취소 뒤 DOM 방식으로 다시 쓰거나 입력을 반복하지 않는다.

성공의 page는 다음 필드를 포함한다.

```json
{
  "ok": true,
  "action": "fill",
  "input": "chromium",
  "attempted": true,
  "dispatchAcknowledged": true,
  "verified": true
}
```

`verified`는 결과를 확인한 시점에 원래 대상의 value가 요청 문자열과 같았다는 뜻이다. HTML validity, 폼 제출, 프레임워크 내부 모델과 서버 저장까지 확인한 결과는 아니다. 입력 뒤 비동기 변경도 가능하다. `trusted:true`나 물리 키보드 입력을 보장하는 필드는 반환하지 않는다.

## 이벤트와 호환성

Chromium이 beforeinput과 input, 값 정규화와 maxlength를 처리한다. 페이지가 beforeinput을 취소하거나 값을 되돌리면 최종 값 검사가 실패할 수 있다. 합성 change를 추가하지 않으며 change의 발생 시점은 포커스 이탈 등 브라우저의 기본 동작에 따른다. 멀티라인 입력은 input 이벤트가 여러 번 발생할 수 있어 한 번의 요청을 한 개의 이벤트로 해석하지 않는다.

일반 Chromium 입력칸과 textarea에는 준비 후 passive beforeinput·input 관찰을 둔다. 각 이벤트 종류의 첫 trusted 이벤트를 관찰하며 원 ref·종류·요청값·읽기 세대와 임시 token을 사후 확인에서 대조한다. 다른 대상에 trusted 이벤트가 전달된 증거가 있으면 원 대상의 값이 이미 요청과 같더라도 `INPUT_TARGET_MISMATCH`로 반환한다. 관찰이 없거나 만료되면 `FILL_STATE_UNCONFIRMED`이며, 관찰 상태가 유효해도 최종 값 검사는 유지한다. 성공의 `inputTarget`은 input 이벤트 대상을 관찰했으면 `matched`, 확인하지 못했으면 `unconfirmed`다. beforeinput만 일치한 경우를 실제 입력 완료로 계산하지 않는다. contenteditable fill은 기존 렌더된 텍스트 검증을 유지한다.

관찰자는 이벤트를 취소하거나 전파를 차단하지 않는다. 사람 입력과 AI 요청에는 DOM 이벤트에서 구분할 수 있는 요청 ID가 없으므로 동시에 발생한 입력을 완전히 구별하지 못한다. 준비 응답의 token은 내부 대조용이며 입력 원문이나 비밀번호를 응답·로그로 추가하지 않는다.

비교는 요청 문자열 그대로다. CRLF가 LF로 정규화되거나 maxlength 때문에 일부만 들어가면 `FILL_STATE_NOT_REACHED`다. 값의 정규화 규칙을 Naru가 재구현하거나 결과를 임의로 정답으로 간주하지 않는다.

email과 number는 실제 Chromium에서 select 뒤 문자열 삽입과 삭제가 관찰됐다. 그러나 selectionStart와 selectionEnd가 null이어서 이번 준비 검사로 전체 선택 상태를 확인할 수 없다. date, range, color는 같은 방식의 문자열 삽입으로 값이 바뀌지 않았다. 기존 지원을 제거하거나 입력 실패 뒤 자동 전환하지 않고, 이 유형들은 처음부터 기존 DOM 경로를 사용한다.

## DOM 경로

기존 지원 유형은 focus 뒤 입력 종류를 고정하고 원문서의 네이티브 value setter, 합성 input, 합성 change 순서로 처리한다. 각 단계 사이에 대상과 편집 조건을 확인하고 마지막 네이티브 getter로 요청 문자열과 비교한다. 성공과 오류의 `trusted:false`는 이 합성 이벤트 경로를 뜻한다. contenteditable의 전체 선택과 네이티브 입력 계약은 별도 문서를 따른다.

## 실패와 입력 중 경합

원래 대상이나 문서, 입력 종류, 편집 조건이나 포커스가 바뀌면 결과를 미확정으로 처리한다. 정상적인 페이지 핸들러가 입력 후 다음 필드로 포커스를 옮긴 경우도 포함한다. 같은 대상의 값만 다르면 `FILL_STATE_NOT_REACHED`다. 요청 취소, 문서 변경이나 엔진 오류도 이미 전송한 입력을 되돌리지 않는다. `attempted`와 `outcomeUnknown`을 보고 현재 페이지를 다시 확인해야 하며, false여도 준비 중 focus나 선택 이벤트의 부작용이 없다는 뜻은 아니다.

DOM 준비와 엔진 전송은 원자적이지 않다. Chromium의 [InsertText 구현](https://github.com/chromium/chromium/blob/156.0.8078.17/content/browser/devtools/protocol/input_handler.cc#L1281-L1303)은 현재 포커스된 widget에 문자열을 전달하며 원 ref를 받지 않는다. 임시 페이지 포커스 준비는 첫 Input의 포커스 전환을 준비 앞으로 옮기는 보완이다. 준비 직후 timer나 다른 입력으로 포커스가 이동하면 다른 대상이 입력을 받을 수 있고 사후 검사로 이미 발생한 변경을 취소하지 못한다. disable 역시 페이지 blur·visibility 핸들러를 실행할 수 있어 그 뒤 검증한다. 백그라운드 AI 탭, 사용자와 동시 입력, 실제 Naru의 포커스 동작은 별도 통합 검증이 필요하다.

요청값과 실제 값, 페이지 예외 원문은 응답이나 오류 상세에 넣지 않는다. 비밀번호도 같은 원칙을 따른다. 요청 문자열, 원 Browser 참조와 작은 준비 상태는 요청 수명 동안만 유지하고 기존 취소와 만료 경로로 해제한다. 새 renderer, worker나 저장 파일은 없다. 임시 포커스 상태는 ACK별 단발 제한과 활성 요청 동안 전역 감시 task 하나를 사용하며 실패 시 요청 본문을 해제한다. 기존 요청 크기와 동시 요청 상한을 사용하며, 전체 브라우저 메모리를 제한하거나 RAM 절감량을 측정한 결과는 아니다.

## 검증 경계

모의 DOM은 준비와 사후 확인을, Rust 단위 검사는 전송과 취소 수명을 검사한다. 독립 ego Chromium의 자체 페이지 관찰은 실제 Naru CEF, macOS 입력이나 외부 서비스 호환성의 증거로 확대하지 않는다. ego의 CDP는 대상 탭을 활성화하므로 Naru의 사용자 선택 보존도 증명하지 못한다.

`tests/native_fill_smoke.py`는 같은 앱 바이너리에서 텍스트와 DOM 경로, 취소와 값 되돌림, Shadow DOM과 iframe, 사용자 선택 보존을 확인하기 위한 검사다. 구문 확인과 실제 실행 결과는 구분한다. 실제 실행 여부, 검증 원본과 생성물 정리는 [작업 기록](work-log.md)에 남긴다.
