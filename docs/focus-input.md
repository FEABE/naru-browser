# Chromium 입력 전 페이지 포커스 준비

2026-10-18 후속 소스의 계약이다. 첫 native 입력이 페이지 포커스를 바꾸면서 뒤늦은 focus handler가 입력 대상을 옮기는 경합을 줄이는 것이 목적이다. `src/browser.rs`에 임시 상태와 정리, `src/dom_bridge.js`에 fill 사전 분류와 이벤트 관찰을 연결했다. 빌드와 실제 실행의 결과는 [작업 기록](work-log.md)을 따르며 소스 조사와 구현을 앱·CEF·E2E 통과로 계산하지 않는다.

## 확인한 문제와 적용 범위

DOM의 `element.focus()`와 `activeElement` 확인이 끝나도 뒤의 `Input.insertText` 또는 `Input.dispatchKeyEvent`는 원래 ref를 받지 않는다. 고정 Chromium은 widget을 선택하고 페이지 포커스를 요청한 뒤 그 시점의 포커스에 입력한다. 첫 페이지 focus 전환에서 실행된 handler가 다른 요소로 포커스를 옮길 수 있으며, 엔진 ACK에는 어느 DOM 요소가 입력을 받았는지가 포함되지 않는다.

`Emulation.setFocusEmulationEnabled(true)`를 요청 동안만 유지해 이 페이지 전환을 새 DOM 준비보다 먼저 처리한다. renderer의 명령 처리는 자신의 Page FocusController를 변경한 뒤 반환한다. local main frame에서는 window·element focus handler가 동기 실행되며, emulation 중의 후속 `SetFocused`는 같은 `FocusHasChanged`를 다시 실행하지 않는다. enable ACK 뒤 원래 ref와 context로 준비를 다시 하면 이 초기 전환에서 생긴 대상 변경을 준비 검사로 확인할 수 있다.

이 범위는 Chromium 텍스트·키 입력 준비다. 기존 check·select 등의 DOM 액션 의미, 사용자 탭 선택과 AI 작업 소유권을 바꾸는 설계가 아니다. 입력 대상 고정이나 모든 focus 경쟁 해결을 뜻하지 않는다. 조사한 공개 Input API에는 DOM node, frame ID, unique context 또는 예상 target을 지정하는 인자가 없다.

fill은 먼저 `inspectOnly`로 종류와 경로만 읽는다. 이후 `expectedInput`·`expectedKind`를 고정하여 실행 전과 focus 후 종류 변경을 거절한다. Chromium 경로만 임시 설정을 사용하고 legacy DOM 입력은 설정 없이 실행하므로 DOM 최종 값 검사가 끝난 뒤 새로운 disable 부작용을 추가하지 않는다. check·select·포인터에는 이 설정을 적용하지 않는다.

## 요청 순서

임시 emulation 상태는 정확한 Browser·session·요청에 묶어 관리한다. 한 번의 입력 흐름은 다음 순서를 따른다.

1. 원래 Browser, 작업 revision·소유권, 탐색·문서 세대, 활성 요청과 원 context를 확인한다.
2. 필요한 정확한 session에 focus emulation enable을 보내고 그 session의 ACK를 기다린다. root ACK로 child session 적용을 대신하지 않는다.
3. ACK 뒤 원래 ref와 context로 새 DOM prepare를 실행한다. 앞서 얻은 준비 결과를 그대로 입력에 사용하거나 새 read로 ref를 재생성하지 않는다.
4. 준비가 유효하면 native 입력을 한 번 시도한다. 텍스트는 기존 `Input.insertText` 한 번, 키는 기존 keyDown/keyUp 한 쌍이다. 각 전송 전후의 Browser·문서·소유권·취소 검사를 유지하고 자동 재시도하지 않는다.
5. 같은 session의 focus emulation disable ACK를 확인한다. 전송 접수만으로 정리됐다고 판단하지 않는다.
6. 원래 context에서 읽기 전용 postverification을 실행한다. 다시 focus하거나 선택을 고쳐 결과를 맞추지 않는다. disable로 생긴 focus·blur 효과도 이 확인보다 앞에 둔다.
7. frame 경로라면 기존 Runtime cleanup ACK까지 처리한 후 최종 응답을 결정한다. 전체 동안 기존 탭 명령 잠금과 요청 수명을 유지한다.

enable 뒤 prepare가 실패했거나 입력 전에 취소됐더라도 disable 책임은 남는다. 입력이 이미 전달됐으면 정리 실패나 취소가 페이지 효과를 되돌리지 못한다. disable 또는 필요한 frame 정리에 실패한 요청을 성공으로 반환하지 않는다. 늦은 ACK는 원래 요청과 정확한 Browser·session에만 적용하며 새 작업의 상태를 해제하는 근거로 사용하지 않는다.

Enter 자체 탐색이나 frame 교체로 원 context를 잃었을 때는 기존 액션의 보수적인 수명·결과 불확실성 정책을 유지한다. 새 문서에서 다시 준비하거나 postverification을 이어서 이전 대상의 성공으로 만들지 않는다.

## 취소와 메모리 상한

탭당 하나, 앱 전체 최대 16개의 임시 상태를 일반 pending과 따로 보관한다. 정확한 Browser·session·token, enable·disable ID와 전송·응답 상태를 유지하며 일반 요청이 취소돼도 해제 책임은 없애지 않는다. 같은 탭의 새 페이지 조작은 상태가 정리될 때까지 거절한다. 닫기·로딩 중지·대기는 기존 제어 경로를 유지한다. 탭이 닫히거나 제어 연결 자체가 없어지면 응답과 상태를 분리해 정리하며 이를 disable ACK를 받은 경우와 구분한다.

각 ACK는 5초 한도를 두며 CEF의 지연 task는 UI 스레드가 실행돼야 처리된다. 활성 상태가 있을 때만 전역 감시 task 하나가 1초마다 기존 45초 요청 수명을 확인한다. 단일 요청마다 장기 감시 루프나 새 스레드를 만들지 않는다. 해제 실패 상태는 요청 텍스트·준비 결과·후속 실행을 버리고 작은 실패 상태만 유지한다. 실패 상태만 남으면 감시 task를 다시 예약하지 않는다. 늦은 정상 ACK 또는 탭·연결 종료까지 해당 탭은 새 조작을 막는다. 프레임 요청의 입력 command와 snapshot도 실패 시 해제하고 필요한 root object group 식별자만 기존 정리를 위해 보관한다.

enable의 네이티브 전송 함수에 재진입해 취소된 경우 disable 송신은 그 함수가 돌아올 때까지 보류한다. enable ACK가 오기 전에 disable을 보냈다면 두 응답을 대조한 뒤 확정된 enable 다음의 disable을 최대 한 번 더 보낸다. 정리 명령은 최대 두 번이며 입력을 반복하는 경로는 없다. 원 session의 해제가 확인되기 전에 내부 frame detach로 책임을 잃지 않도록 기존 frame 추적 정리도 보류한다.

fill의 이벤트 관찰은 insert와 bridge당 하나를 공유한다. 일반 입력칸과 textarea의 beforeinput·input 각 첫 trusted 이벤트만 passive capture하며 최대 네 listener, 5초 단발 만료를 사용한다. 새 읽기·새 동작·결과 소비에서도 해제한다. Event 객체나 강한 요소 참조를 별도 이력으로 보관하지 않지만 관찰 함수의 짧은 수명 동안 원 대상과 문맥을 참조한다. renderer가 멈추면 만료도 늦어질 수 있다. 상한과 정리는 전체 Chromium RAM 절감의 측정 결과가 아니다.

## 네이티브 포커스와 페이지 상태의 차이

이 설계는 AppKit first responder, NSWindow 활성화, Naru의 공간이나 선택 탭을 변경하는 호출을 추가하지 않는다. 조사한 Emulation 처리 경로에도 `makeFirstResponder`나 OS 창 활성화 요청은 없다. 이것은 선택한 구현 경로의 소스 범위이며 실제 Naru 화면에서 포커스 보존을 확인한 결과는 아니다.

`CefBrowserHost.set_focus(true)`는 사용하지 않는다. 고정 CEF의 기본 Alloy 경로는 macOS native delegate를 거쳐 콘텐츠 NSView를 `makeFirstResponder`로 지정한다. 따라서 주소 편집기나 현재 사용자의 입력 포커스를 보존하는 준비 함수로 쓸 수 없다. CEF focus callback이 이 기본 경로를 막았다는 사실도 renderer의 포커스 준비 완료 증거가 되지 않는다.

반면 focus emulation은 페이지에 보이는 상태를 바꾼다. effective active/focused는 실제 값과 emulation flag의 OR이며, browser 쪽은 `stay_hidden:false` capture handle을 유지해 페이지 visibility도 visible로 계산할 수 있다. focus·blur·visibility handler가 실행되거나 페이지 로직이 달라질 수 있다. 이를 부작용 없는 대기 장벽이라고 설명하지 않는다.

disable은 저장해 둔 예전 포커스나 DOM 선택을 되돌리지 않는다. 그 시점의 실제 `is_focused`·`is_active` 상태로 돌아가며 blur가 발생할 수 있다. 그동안 native Input이나 사람이 바꾼 실제 상태를 무조건 덮어쓰는 복원 단계를 넣지 않는다. 요청 종료 때 mode를 해제하되 페이지의 변화 자체를 되돌렸다고 주장하지 않는다.

## root와 child의 경계

Emulation의 renderer 처리는 해당 session이 연결한 Page에 적용된다. root session의 ACK는 모든 OOPIF renderer, focus handler와 비동기 작업이 끝났다는 전역 신호가 아니다. 입력 대상 child session이 있으면 해당 session의 적용과 정리 증거를 별도로 관리해야 한다.

remote main frame을 가진 Page는 emulation flag를 설정하지만 즉시 `FocusHasChanged`를 실행하지 않는다. effective focus와 이후 focus 이벤트 억제는 적용되더라도 local main frame과 똑같은 이벤트 순서를 가정하지 않는다. 실제 mixed-process·OOPIF 전파와 background 입력은 통합 E2E에서 확인해야 한다.

문서·session 교체 이후에도 같은 emulation 상태가 새 renderer에 적용될 수 있는 엔진 경로가 있다. 따라서 취소나 탐색을 단순히 응답 버리기로 처리해 임시 mode를 남겨서는 안 된다. 정확한 session의 disable, 또는 그 session의 폐기와 기존 정리 정책을 구분해 처리해야 한다. root 연결이 살아 있는 경우 child detach만으로 root mode까지 해제됐다고 보지 않는다.

정확한 부모가 보고한 `Target.detachedFromTarget`에서 registry가 제거한 subtree에 임시 상태의 child session이 포함되면 해당 입력을 `FRAME_CHANGED`, `focusCleanup:session-destroyed`로 끝낸다. 원 pending과 임시 상태를 해제하고 기존 frame 정리를 이어간다. root lease나 다른 subtree의 lease를 이 알림으로 해제하지 않는다. Chromium의 session 소멸은 browser·renderer agent의 disable 경로로 이어지지만 detached 알림이 dispose보다 먼저 보내지고 renderer 해제도 비동기다. 따라서 이를 명시적 disable ACK나 renderer reset 완료로 기록하지 않는다.

## 남는 경쟁과 성공 의미

Runtime 평가와 native 입력은 별도 요청·큐 경로다. browser UI 턴에서 연속 enqueue해도 하나의 renderer task로 합쳐지는 공개 계약은 없다. Runtime은 평가 후 microtask를 실행하지만 Input은 별도 widget main-thread 경로를 사용한다. focus handler가 예약한 timer·rAF, 다른 frame 이벤트와 사람 입력을 모두 비우는 장벽도 아니다.

따라서 enable ACK → prepare 이후에도 포커스·DOM·문서가 바뀔 수 있다. postverification은 원래 대상의 결과나 입력 관찰을 확인하고 불일치를 보고하는 단계이며, 원치 않은 입력을 항상 예방하거나 취소하지 않는다. `attempted`·결과 불확실성·각 액션의 성공 필드는 기존 계약을 유지한다. disable ACK와 엔진 입력 ACK만으로 제출·저장·서버 처리 성공을 판단하지 않는다.

Chromium 내부 IME에는 target DOM node 인자가 있지만 현재 CDP `InsertText`는 빈 node ID를 넘긴다. 공개 CEF/CDP 옵션으로 이 대상을 고정하는 구현은 확인하지 못했다. 엔진 확장이나 DOM 합성 입력은 별도 설계이며 이 변경에 포함하지 않는다.

## 근거와 남은 확인

[focus emulation 소스 조사](../artifacts/focus-emulation-v1-sources.json)에 Chromium `156.0.8078.17`, CEF `8db2f36735f06aacaf1bab832cb08da9d9445c55`, V8 `4b33f15d4d172248a95629d6aa9917b20a79ad76`의 URL·해시·핵심 발췌와 해석 한계를 기록했다. 기존 [frame 입력 조사](../artifacts/frame-input-v1-sources.json)의 widget 포커스·ACK 경계도 재사용했다. 원문 확인과 문서 정합성 검사는 실제 실행 증거가 아니다.

후속 검증에서는 첫 native 입력의 지연 focus 전환, enable/disable handler가 대상을 바꾸는 경우, root·OOPIF별 ACK, 중간 취소·인계·문서 교체와 정리 실패, 늦은 응답을 확인해야 한다. 동시에 사용자 공간·선택 탭·주소 편집기·OS 포커스와 키체인 동작을 별도로 확인한다. 검증 결과와 실행·정리 이력은 [작업 기록](work-log.md)에 구분해 남긴다.
