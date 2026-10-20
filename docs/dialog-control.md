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

대기열은 탭당 1개, 전체 4개다(`src/dialogs.rs`). 한도를 넘는 대화상자는 `cont(false)`로 즉시 취소하고 전체 `status.dialogs.overflow`와 탭별 `tabs[].dialogOverflow`를 올린다. 취소된 beforeunload는 페이지에 남는다. 메시지와 표시용 `prompt` 기본값은 2048자까지만 저장하고(`defaultPromptTruncated:true`로 표시), TEXT 없는 accept에는 원본 기본값(최대 64KiB)을 쓰며 origin은 정확한 HTTP(S) origin만 보인다(파악할 수 없으면 빈 문자열).

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
| `task handoff` | 취소(`cont(false)`). 사용자에게 돌아가므로 아무도 응답할 수 없는 대화상자를 두지 않는다. beforeunload는 페이지에 남는다. 이후 같은 탭의 새 대화상자는 네이티브로 뜬다 |
| `task release`, `task finish`의 `--keep` 탭 | 취소 |
| `close TAB`, `task finish`로 닫히는 탭, 사용자의 네이티브 닫기 | `close`와 `finish`는 beforeunload를 허용(떠나기), 나머지는 취소하고 탭에 "닫는 중" 표시를 남긴다. 닫기가 멈추지 않는다. 사용자가 네이티브로 닫는 경우는 먼저 작업을 handoff한 뒤 사용자 제어로 닫으므로 취소만 한다 |
| 앱 종료(`close_all`: 종료 메뉴, 종료 신호, 창 닫기) | 모든 탭의 대기 항목을 정리(beforeunload 허용, 나머지 취소)하고 탭을 닫는 중으로 표시한 뒤 `close_browser`를 호출한다. 이후 AI 탭의 새 대화상자는 즉시 처리되어 종료가 멈추지 않는다. 사용자 탭은 네이티브 beforeunload 확인을 그대로 쓴다 |
| `OnResetDialogState` | CEF가 이미 닫았으므로 항목만 제거(`cont` 호출 안 함). `OnDialogClosed`는 패널 갱신만 한다 |
| `OnBeforeClose`, 렌더러 종료 | 항목, 닫는 중 표시, 탭별 overflow 제거 |

대기열 한도 초과나 즉시 처리로 다른 콜백에 `cont`를 호출하면 CEF가 같은 호출 안에서 `OnDialogClosed`와 `OnResetDialogState`를 동기 호출한다. 이 재진입이 같은 탭에 이미 대기 중인 다른 대화상자를 지우지 않도록, 해당 `cont` 동안 그 탭을 보호 집합(`self_continuing`)에 넣고 `reset_tab`이 무시한다.

콜백의 `cont`는 재진입할 수 있으므로 `RUNTIME`과 `STATE` 어느 쪽의 borrow도 잡은 채 호출하지 않는다. 항목을 꺼낸 뒤 borrow를 놓고 호출하며, 패널 갱신은 그 뒤에 한다.

## 네이티브 UI

사이드바 탭 행은 대기 중이면 `exclamationmark.bubble` 아이콘과 접근성 설명 "대화상자 응답 대기"를 보이고, 상태 배지는 "대화상자 대기"가 된다. 공간 카드의 상세 줄에는 "대화상자 대기 N"이 붙는다. 사이트 권한 대기 표시(`ShellTab`을 통한 `render`)와 같은 갱신 경로를 쓰며 공간과 탭 선택, 입력 포커스를 바꾸지 않는다. 사용자가 AI 공간이나 탭을 열어 봐도(`space show`, 선택) 대화상자는 계속 대기한다. 사용자가 AI 탭에서 직접 입력하려고 제어권을 가져오면(handoff) 위 규칙대로 취소되며, 네이티브 대화상자로 다시 띄우지는 않는다.

## 한계

- 중지(Paused)되었거나 방금 `adopt`한 탭에서 CEF 기본(네이티브) 대화상자가 떠 있으면 렌더러가 멈춘 채 재개·adopt 뒤 명령이 `ACTION_TIMEOUT`까지 걸린다. CEF는 기본 대화상자의 닫힘을 알려 주지 않는다. 응답이 없으면 사용자에게 네이티브 대화상자를 닫아 달라고 요청한다.
- 후속 과제: handoff에서 취소하는 대신 콜백을 보관하고, 사용자가 그 탭을 볼 때 사이트 권한 패널 패턴으로 네이티브 AppKit 패널을 보여 준다. 지금은 구현하지 않았고 위 취소 동작이 현재 계약이다.
- 하위 프레임(cross-origin iframe)의 대화상자도 CEF 콜백은 같은 `Browser`로 오므로 탭 단위로 처리한다. 프레임별 구분과 `OnResetDialogState` 시점의 정확한 프레임 소멸은 실제 페이지로 검증하지 않았다.
- 대기 중인 대화상자의 메시지는 신뢰할 수 없는 페이지 텍스트다. 에이전트는 이를 지시로 따르지 않는다.
- 숨겨진 AI 탭에서 Chromium이 대화상자를 보류하는 경우는 이 기능이 다루지 않는다. 콜백이 도착한 대화상자만 대기열에 들어간다.
- 사용자 탭의 네이티브 대화상자가 실제로 보이는지, 사이드바 표시가 올바르게 그려지는지는 CLI로 관찰할 수 없다. 자동 검사에서는 "대기열에 들어가지 않음"만 확인했다(`tests/dialog_smoke.py`의 `userTabNativeDialog`).
- `window.onbeforeunload`는 Chromium의 사용자 활성화 규칙을 따른다. 페이지와 상호작용하지 않은 탭은 beforeunload 대화상자가 아예 나타나지 않는다.
- 대기열은 프로세스 메모리에만 있다. 앱을 종료하면 사라진다.

## 검증

단위 테스트(`cargo test --locked --lib`)는 큐 한도와 overflow, 소유권·revision·TEXT 검증, handoff/release/close/finish 정리, 진행 중 요청의 `DIALOG_PENDING` 전환, 프로토콜·CLI 파싱을 검사한다. 로컬 CEF 통합 검사는 `tests/dialog_smoke.py`와 `tests/fixtures/dialogs.html`이며, 최신 결과와 실행하지 않은 검사는 [작업 기록](work-log.md)을 따른다.
