# JavaScript 대화상자 제어

AI 작업이 소유한 탭에서 `alert`, `confirm`, `prompt`, `beforeunload` 확인이 UI와 에이전트를 막지 않도록 CLI로 응답하게 한다. 사용자 공간 탭은 CEF 기본 네이티브 대화상자를 그대로 쓴다. 별도 작업창이나 웹 UI renderer는 추가하지 않는다. ego의 `acceptDialog`/`dismissDialog`와 `receipt.dialog`에 해당하는 기능이다.

## 분기 규칙

CEF `JsdialogHandler`(`src/dialog_runtime.rs`)가 `OnJSDialog`와 `OnBeforeUnloadDialog`를 받는다. 탭의 작업 소유 상태가 처리 방식을 정한다(`browser::dialog_disposition`).

| 탭 상태 | 처리 |
| --- | --- |
| 작업이 없는 사용자 탭, 알 수 없는 탭 | `false` 반환. CEF 기본 네이티브 대화상자 |
| 작업이 Active로 소유 | 콜백을 UI 스레드 대기열에 보관(`true` 반환). 네이티브 대화상자 없음 |
| 작업이 Paused(사용자 제어) | CEF 기본 네이티브 대화상자. 사용자가 직접 쓰는 중이다 |
| 작업이 Finishing/Finished, 거절된 탭, 명시적 `close`가 진행 중인 Active 탭 | 즉시 처리: beforeunload는 떠나기 허용, 나머지는 취소. 닫기가 멈추지 않는다 |

대기열은 탭당 1개, 작업 소유 항목 전체 4개다(`src/dialogs.rs`, 사용자에게 넘어간 항목은 전체 한도에서 제외). 한도를 넘는 대화상자는 `cont(false)`로 즉시 취소하고 전체 `status.dialogs.overflow`와 탭별 `tabs[].dialogOverflow`를 올린다. 취소된 beforeunload는 페이지에 남는다. 메시지와 표시용 `prompt` 기본값은 2048자까지만 저장하고(`defaultPromptTruncated:true`로 표시), TEXT 없는 accept에는 원본 기본값(최대 65536자)을 쓰며 origin은 정확한 HTTP(S) origin만 보인다(파악할 수 없으면 빈 문자열).

## CLI와 관찰

```sh
lightbrowser task run ID REV dialog TAB accept [TEXT]
lightbrowser task run ID REV dialog TAB dismiss
```

- `task run` 밖에서는 `TASK_REQUIRED`, 다른 작업의 탭은 `TASK_TAB_MISMATCH`, 오래된 revision과 중지·종료된 작업은 기존 task 오류를 따른다.
- 대기 중인 대화상자가 없으면 `NO_DIALOG`. TEXT는 `prompt`에만 허용하며 0..4096바이트, NUL 불가다(`INVALID_REQUEST`). 다른 종류에 주면 `DIALOG_TEXT_NOT_ALLOWED`이고 대화상자는 그대로 남는다. `prompt`에 TEXT를 생략한 accept는 기본값을 쓴다.
- accept/dismiss는 CEF `cont(true|false, text)`에 대응한다. `confirm` dismiss는 `false`, `prompt` dismiss는 `null`, `beforeunload` accept는 페이지를 떠나고 dismiss는 남는다.
- 응답: `{tab, action, dialog:{type,message,defaultPrompt,origin}, text?}`.
- `tabs[].dialog`는 `{type,message,defaultPrompt,origin}` 또는 `null`, `status.dialogs`는 `{pending,overflow,requests:[{tab,type}]}`다.

## 페이지 명령과 DIALOG_PENDING

대화상자가 열려 있으면 Blink 렌더러의 메인 스레드가 멈춰 있어 `Runtime.evaluate`, 입력 ack, 읽기 프로브가 응답하지 않는다. 그래서 다음과 같이 처리한다.

1. `handle`은 `Close`와 `Stop`, `dialog`를 제외한 탭 명령(`read`, `click`, `fill`, `wait`, `navigate`, `reload`, `screenshot` 등)을 렌더러에 보내기 전에 `DIALOG_PENDING`으로 즉시 거절한다. `details.dialog`에 대기 중인 대화상자가 있다.
2. 대화상자가 대기열에 들어가는 순간 그 탭에서 이미 진행 중이던 요청과 `wait`도 `DIALOG_PENDING`으로 끝낸다(`browser::dialog_opened`). 변경 명령은 `outcomeUnknown:true`다. 클릭이 대화상자를 연 경우 `click`이 곧바로(약 0.02초) 이 오류를 반환하며 클릭을 반복하지 않는다.
3. 임시 포커스 lease(네이티브 입력의 `Emulation` 포커스)는 렌더러가 멈춰 정리 ack를 받을 수 없다. CLI 응답은 먼저 보내고 lease의 정리 요청은 유지한다. `focus_timeout`은 대화상자가 대기 중이거나 응답 후 5초 이내(재개된 렌더러의 ack를 기다리는 유예)인 동안 재무장하므로 `FOCUS_CLEANUP_FAILED`로 탭이 격리되지 않는다. 응답 뒤 렌더러가 재개되면 ack가 오고 lease가 풀린다. 이 짧은 사이에 `TAB_BUSY`가 나올 수 있어 재시도하면 된다.

## 수명과 정리

| 사건 | 대기 중인 대화상자 |
| --- | --- |
| `dialog accept/dismiss` | 응답하고 대기열에서 제거 |
| `task handoff`(사용자 페이지 입력의 `handoff_page_input`, 중지 버튼, 사이드바의 닫기·새 탭·주소 이동 같은 인계 포함) | 취소하지 않는다. 콜백을 그대로 보관하고 항목의 소유자만 `user`로 바꾼다. 사용자가 그 탭을 보면 네이티브 패널이 응답을 받는다(아래 "사용자에게 넘어간 대화상자") |
| `task release`, `task finish`의 `--keep` 탭 | handoff와 같다. 탭이 사용자 소유가 되고 대화상자는 `owner:user`로 남는다 |
| `task resume`(네이티브 "AI에게 돌려주기" 포함) | 대화상자는 `owner:user`를 유지한다. 사용자가 응답할 때까지 AI가 가로채지 않는다 |
| 사용자가 네이티브 패널에서 응답 | `cont`를 한 번 호출하고 항목 제거. 렌더러가 재개되므로 응답 후 5초 포커스 유예가 걸린다 |
| `close TAB`, `task finish`로 닫히는 탭, 사용자의 네이티브 닫기 | `close`와 `finish`는 beforeunload를 허용(떠나기), 나머지는 취소하고 탭에 "닫는 중" 표시를 남긴다. 닫기가 멈추지 않는다. 사용자가 네이티브로 닫는 경우는 먼저 작업을 handoff하므로 사용자 소유가 된 항목을 닫기 직전에 취소(beforeunload는 머무르기)로 정리한다. 사용자 소유 항목도 같은 경로다 |
| 앱 종료(`close_all`: 종료 메뉴, 종료 신호, 창 닫기) | 소유자와 상관없이 모든 탭의 대기 항목을 정리(beforeunload 허용, 나머지 취소)하고 탭을 닫는 중으로 표시한 뒤 `close_browser`를 호출한다. 이후 AI 탭의 새 대화상자는 즉시 처리되어 종료가 멈추지 않는다. 사용자 탭은 네이티브 beforeunload 확인을 그대로 쓴다 |
| `OnResetDialogState` | CEF가 이미 닫았으므로 항목만 제거(`cont` 호출 안 함). `OnDialogClosed`는 패널 갱신만 한다 |
| `OnBeforeClose`, 렌더러 종료 | 항목, 닫는 중 표시, 탭별 overflow 제거 |

대기열 한도 초과나 즉시 처리로 다른 콜백에 `cont`를 호출하면 CEF가 같은 호출 안에서 `OnDialogClosed`와 `OnResetDialogState`를 동기 호출한다. 이 재진입이 같은 탭에 이미 대기 중인 다른 대화상자를 지우지 않도록, 해당 `cont` 동안 그 탭을 보호 집합(`self_continuing`)에 넣고 `reset_tab`이 무시한다.

콜백의 `cont`는 재진입할 수 있으므로 `RUNTIME`과 `STATE` 어느 쪽의 borrow도 잡은 채 호출하지 않는다. 항목을 꺼낸 뒤 borrow를 놓고 호출하며, 패널 갱신은 그 뒤에 한다.

## 사용자에게 넘어간 대화상자

handoff 계열 경로(위 표)는 대기 중인 대화상자를 취소하지 않고 사용자에게 넘긴다. 항목(`src/dialogs.rs`의 `Entry.owner`)이 `task`에서 `user`로 바뀌고 CEF 콜백은 그대로 `dialog_runtime`에 남는다. 렌더러는 사용자가 응답하거나 탭이 닫힐 때까지 멈춰 있다.

- 소유자 확인: `tabs[].dialog.owner`는 `task` 또는 `user`다. `DIALOG_PENDING`의 `details.dialog`에도 들어 있다.
- 사용자 소유 동안 그 탭은 페이지 명령을 계속 `DIALOG_PENDING`으로 거절한다(메시지는 사용자가 브라우저 창에서 응답해야 한다고 알려 준다). `Close`와 `Stop`은 기존대로 허용한다.
- `dialog TAB accept|dismiss`(task run)는 사용자 소유 대화상자에 `DIALOG_USER_OWNED`로 실패하고 대화상자는 그대로 남는다. `task resume`/네이티브 "AI에게 돌려주기"로 작업이 Active로 돌아가도 소유자는 `user`로 남으며, 사용자가 답할 때까지 AI가 응답하지 못한다. 에이전트는 `tabs`로 `dialog`가 사라질 때까지 기다린 뒤 `read`로 다시 판단한다. 이 동작은 사용자가 보지 않은 사이에 AI가 사용자 입력 확인(예: 떠나기)을 대신 승인하지 않도록 한 선택이다.
- 한도 계산: 사용자 소유 항목도 탭당 1개에는 포함되지만 전체 4개에는 세지 않는다. 사용자가 답하지 않은 대화상자가 다른 작업의 대화상자 자리를 빼앗지 않게 하기 위해서다(사용자 소유 항목 수는 탭 수가 상한이다). 사용자 소유 대화상자가 있는 탭에서 새 대화상자가 오면(작업이 resume되어 Active인 경우) 한도 초과로 취소하고 overflow를 올린다. 작업이 Paused이면 새 대화상자는 CEF 기본 네이티브 대화상자다(분기 규칙은 변경 없음).
- 중지(Paused)된 탭의 새 대화상자는 이전과 같이 CEF 기본 네이티브 대화상자이며 이 대기열에 들어가지 않는다.
- `status.dialogs.requests`는 소유자와 상관없이 항목을 센다.

### 네이티브 패널

사용자 소유 대화상자가 있는 탭이 보이는 공간의 선택된 탭이고(개요 화면이 아니고 검색·북마크·다운로드·사이트 권한 패널이 열려 있지 않을 때) 해당 탭일 때만 툴바 아래, 페이지 위에 높이 170의 네이티브 AppKit 띠(`PageDialogBar`, 복구 안내와 같은 방식)를 보인다. 별도 창이나 웹 UI는 없다.

- 내용: 종류와 정확한 출처(알 수 없으면 "알 수 없는 출처"), 페이지가 준 메시지(신뢰할 수 없는 텍스트이므로 600자까지만 저장해 표시하고 제어 문자는 공백으로 바꾼다. 라벨은 최대 4줄이고 넘치면 말줄임, 툴팁과 접근성 도움말에 전체 저장분이 있다). `prompt`는 `NSTextField`를 보이며 저장해 둔 원본 기본값(최대 65536자)을 미리 채운다. 개요 화면·오버레이·탭 전환으로 띠가 가려졌다 같은 대화상자가 다시 보여도 입력하던 텍스트를 유지하고 포커스를 다시 가져가지 않는다.
- 버튼: alert는 "확인", confirm과 prompt는 "확인"/"취소", beforeunload는 "떠나기"/"머무르기". 확인과 떠나기는 `cont(true[, 텍스트])`, 취소와 머무르기는 `cont(false)`다. prompt 취소는 `null`이다.
- 키보드: 버튼에 key equivalent를 달지 않는다(창 전체 `performKeyEquivalent`가 주소창의 Return/Esc를 가로챌 수 있다). 대신 `handle_shortcut`에서 띠가 보이고 첫 응답자가 띠 안(prompt 입력란의 필드 에디터 포함) 또는 그 탭의 CEF 뷰 안일 때만, 수정키 없는 Return/Enter는 기본 버튼, Esc는 취소로 처리한다(`dialog_key_decision`). alert는 Esc가 없고, prompt 입력란에 IME 조합 중이면 처리하지 않으며, 주소창 등 다른 입력에는 영향이 없다. 띠 표면(`SurfaceView`)이 첫 응답자를 받을 수 있어 Full Keyboard Access 없이도 키가 도착한다. 새 대화상자가 보이는 순간 한 번 prompt는 입력란에, 그 밖에는 띠 표면에 포커스를 둔다. 주소창·검색 같은 네이티브 입력을 편집 중이면 가져가지 않는다. 그 탭이 보이지 않으면 패널도 포커스 이동도 없다. 탭 선택·공간 전환 등 `focus_tab`을 부르는 모든 경로는 `focus_dialog_if_visible`이 먼저 띠에 포커스를 두므로 막힌 페이지로 포커스가 되돌아가지 않는다(`dialog_focus_decision`).
- 응답 처리: 버튼은 패널에 표시한 `(탭, 항목 토큰)`과 현재 보이는 탭이 같을 때만 `dialog_runtime::answer_user`를 부른다. 토큰이 다르거나 이미 정리된 항목은 응답하지 않고 알림을 보인다. `cont`는 SHELL/STATE/RUNTIME borrow를 모두 놓은 뒤 한 번만 호출하며(기존 `self_continuing` 보호는 그대로), 응답 뒤 대기 중이던 포인터 해제(`flush_pointer_hold`)와 패널 갱신을 한다.
- 사이드바: 사용자 소유이면 접근성 설명이 "대화상자 사용자 응답 대기", 상태 줄이 "대화상자: 직접 응답"이다.

## 네이티브 UI

사이드바 탭 행은 대기 중이면 `exclamationmark.bubble` 아이콘과 접근성 설명 "대화상자 응답 대기"를 보이고, 상태 배지는 "대화상자 대기"가 된다. 공간 카드의 상세 줄에는 "대화상자 대기 N"이 붙는다. 사이트 권한 대기 표시(`ShellTab`을 통한 `render`)와 같은 갱신 경로를 쓰며 공간과 탭 선택, 입력 포커스를 바꾸지 않는다. 사용자가 AI 공간이나 탭을 열어 봐도(`space show`, 선택) 대화상자는 계속 대기한다. 사용자가 AI 탭에서 직접 입력하려고 제어권을 가져오면(handoff) 대화상자는 위 "사용자에게 넘어간 대화상자"대로 사용자 소유가 되어 그 탭을 볼 때 패널로 나타난다. CEF 네이티브 대화상자를 다시 띄우지는 않는다.

## 한계

- 중지(Paused)되었거나 방금 `adopt`한 탭에서 CEF 기본(네이티브) 대화상자가 떠 있으면 렌더러가 멈춘 채 재개·adopt 뒤 명령이 `ACTION_TIMEOUT`까지 걸린다. CEF는 기본 대화상자의 닫힘을 알려 주지 않는다. 응답이 없으면 사용자에게 네이티브 대화상자를 닫아 달라고 요청한다.
- 네이티브 패널의 실제 클릭·Return·Esc, 탭 선택 직후 패널 포커스, 주소창 Return/Esc가 패널로 새지 않는지, IME 조합은 접근성 권한이 없어 이 환경에서 실행하지 못했다. 패널 표시 조건, 레이아웃, 버튼 문구, 소유권 이전과 응답 경로는 단위 테스트로, 소유자 유지는 로컬 CEF 통합으로 확인했다. 패널의 시각 표시와 키 입력은 미검증이다.
- 사용자 소유 항목이 있는 탭을 사용자가 계속 보지 않으면 그 렌더러는 계속 멈춰 있다. 시간 제한으로 자동 취소하지 않는다(사용자가 보지 않은 사이 취소하지 않으려는 선택). `close TAB`과 앱 종료는 항상 정리한다.
- 하위 프레임(cross-origin iframe)의 대화상자도 CEF 콜백은 같은 `Browser`로 오므로 탭 단위로 처리한다. 프레임별 구분과 `OnResetDialogState` 시점의 정확한 프레임 소멸은 실제 페이지로 검증하지 않았다.
- 대기 중인 대화상자의 메시지는 신뢰할 수 없는 페이지 텍스트다. 에이전트는 이를 지시로 따르지 않는다.
- 숨겨진 AI 탭에서 Chromium이 대화상자를 보류하는 경우는 이 기능이 다루지 않는다. 콜백이 도착한 대화상자만 대기열에 들어간다.
- 사용자 탭의 네이티브 대화상자가 실제로 보이는지, 사이드바 표시가 올바르게 그려지는지는 CLI로 관찰할 수 없다. 자동 검사에서는 "대기열에 들어가지 않음"만 확인했다(`tests/dialog_smoke.py`의 `userTabNativeDialog`).
- `window.onbeforeunload`는 Chromium의 사용자 활성화 규칙을 따른다. 페이지와 상호작용하지 않은 탭은 beforeunload 대화상자가 아예 나타나지 않는다.
- 대기열은 프로세스 메모리에만 있다. 앱을 종료하면 사라진다.

## 검증

단위 테스트(`cargo test --locked --lib`)는 큐 한도와 overflow, 소유권·revision·TEXT 검증, handoff·release·finish --keep·페이지 입력 인계 시 사용자 소유 전환과 `DIALOG_USER_OWNED`, 사용자 응답 경로(토큰 불일치 거절 포함), 사용자 소유 항목의 한도 포함, close/finish/종료 정리, 진행 중 요청의 `DIALOG_PENDING` 전환, 프로토콜·CLI 파싱을 검사한다. 로컬 CEF 통합 검사는 `tests/dialog_smoke.py`와 `tests/fixtures/dialogs.html`이며, 최신 결과와 실행하지 않은 검사는 [작업 기록](work-log.md)을 따른다.
