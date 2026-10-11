# HTML 단일 선택 제어

기준일: 2026-10-11. 기본 HTML `<select>`를 CLI로 읽고 선택하는 소스를 추가했다. select v1 release부터 앱과 CLI에 명령이 포함됐다. 사용자의 키체인 승인 뒤 같은 바이너리에서 실제 선택값·화면·이벤트·제어권을 확인했다. 배경 focus 가정을 고친 뒤 동일 바이너리에서 선택 10개와 기존 회귀 22개가 통과했다.

## 사용과 응답

새 소스로 앱과 CLI를 함께 빌드한 뒤 `read TAB`이 반환한 최신 ref와 `options[].value`를 사용한다.

```text
select TAB REF VALUE
task run ID REV select TAB REF VALUE
```

VALUE는 옵션의 정확한 값이다. 빈 문자열, 앞뒤 공백과 한글을 보존하며 표시 이름으로 대신 선택하지 않는다. 값 상한은 JavaScript 문자열과 같은 UTF-16 코드 단위 1,024개다. Rust CLI·원본 소켓 요청·작업 범위 요청과 DOM 영역이 같은 상한을 적용한다. 기존 `interactive[].value`는 표시를 위해 축약될 수 있으므로 명령에는 옵션의 정확한 `value`를 사용한다.

읽기 결과의 native SELECT에 `multiple`, `options`, `optionsTruncated`가 추가된다. 옵션은 `selected`, `disabled`, `hidden` 상태를 가진다. 숨기지 않은 옵션은 정확한 `value`와 최대 240자의 표시 `label`을 포함한다. 너무 긴 값은 `value`를 생략하고 `valueTooLong:true`로 표시한다. 숨김 옵션은 label/value를 읽지 않으며 상태만 반환한다. 현재 선택값의 기존 표시 동작은 유지한다.

선택 성공은 `page.ok:true`, `page.action:"select"`, `page.trusted:false`, `page.changed`를 반환한다. 값이 실제로 다른 경우 native selectedIndex setter를 호출하고 합성 `input`·`change` 이벤트를 순서대로 보낸다. 이미 선택된 옵션이면 포커스·값·이벤트를 바꾸지 않는다. 페이지 처리기가 값을 다시 바꾸거나 탐색·제출할 수 있으므로 성공 응답은 최종 페이지 결과의 보장이 아니며 이후 `read`나 `wait`로 확인한다.

## 거절과 제어권

| 상황 | 결과 |
| --- | --- |
| multiple SELECT 또는 사용자 정의 combobox | `UNSUPPORTED_ELEMENT` |
| 정확히 일치하는 값 없음 | `OPTION_NOT_FOUND` |
| 같은 값의 옵션 둘 이상 | `AMBIGUOUS_OPTION` |
| 대상 옵션·optgroup이 숨김 또는 disabled | `OPTION_UNAVAILABLE` |
| SELECT 자체가 숨김·inert·disabled | `ELEMENT_UNAVAILABLE` |
| 포커스 처리 중 SELECT 또는 대상 옵션이 교체됨 | `ELEMENT_UNAVAILABLE` |
| SELECT의 옵션이 500개 초과 | `OPTION_LIMIT` |
| 1,024 UTF-16 상한 초과 | 소켓의 `INVALID_REQUEST`, CLI의 `INVALID_ARGUMENTS` 검증 실패 또는 DOM의 `INVALID_VALUE` |
| 문서·read 세대·탐색 epoch가 달라진 ref | 기존 `STALE_REF` 경로 |

선택 전에 검사하고, focus 처리기가 실행된 뒤 같은 노드와 옵션을 다시 검사한다. 옵션 자체는 닫힌 native 메뉴에서 레이아웃 박스가 없을 수 있어 SELECT의 표시 상태와 옵션·조상 상태로 가용성을 판정한다. 숨김 또는 비활성 옵션을 임의로 활성화하지 않는다.

`Command::tab_id → handle의 tasks.check/check_tab → dispatch의 check_tab → devtools_result의 scope/epoch/활성 요청 검사 → DOM act`를 따른다. 기존 작업 revision·중지·취소 경로를 재사용하며 자동 재시도하지 않는다. 이미 전달된 변경 요청의 취소는 결과 불확실성을 보존한다. 추가 창·프로필·저장소·상주 worker는 없다.

## 자원과 검증 경계

옵션 읽기는 SELECT당 처음 40개, 한 번의 read 전체 240개까지다. 나머지는 `optionsTruncated:true`이며 이 버전에는 옵션 페이지 이동 API가 없다. 선택은 옵션 500개 이하인 SELECT만 검사하고 focus 전후 최대 두 번 확인한다. 전체 옵션 배열을 복제하거나 옵션별 ref를 보관하지 않는다. ref가 가리키는 SELECT 노드와 기존 read 수명주기를 공유한다. 실제 Chromium 페이지 자체의 DOM 메모리 사용량은 이 상한과 별개다.

Rust 검사 148개가 통과했다. 새 검사는 정확값과 Unicode 상한의 CLI·작업·원본 소켓 왕복, 명령 형식 거절, 이전 epoch 거절과 JSON 데이터 직렬화를 포함한다. 초기 원본 소켓 검사는 task 명령 허용 목록 누락을 찾아냈고 추가 후 통과했다. CLI가 제어문자 ref를 파싱 단계에서 모두 거절한다는 초기 테스트 가정은 기존 구현과 달라 제거했다. 실제 페이지 실행 전의 기존 ref 검증은 유지한다.

모의 DOM 검사 37개가 통과했다. 옵션 노드·SELECT 교체, 가시성·disabled optgroup, 중복·빈 값·공백, stale ref, 상한, native setter, 이벤트 순서·동일값 무동작을 포함한다. 이는 CEF나 외부 사이트의 실제 폼 검증을 대신하지 않는다. 로컬 CEF용 `tests/select_smoke.py`의 최초 실행은 배경 focus 이벤트에 대한 잘못된 가정에서 중단됐다. 독립 진단 뒤 배경 선택과 전경 focus 교체를 구분한 수정 검사에서 선택 10개와 기존 회귀 22개가 통과했다. 근거는 `artifacts/select-v1-corrected-suite.json`과 `-state.json`이다. iframe·닫힌 shadow root·커스텀 combobox·다중 선택·trusted native 입력은 이번 구현 범위에 포함되지 않는다.

## 실행 상태

select v1 앱 SHA-256은 `8a7e81db40966fecbe982661cb78fc6a0a6ffdb38dfb5ebf450117dc123548e8`다. `artifacts/select-v1-build.log`에서 release 성공과 Cargo 중간 파일 1,263개·670.0MiB 정리를 확인했다. 이전 재열기 v2 앱은 정상 종료 exit 0 뒤 교체했다. 첫 로컬 탭의 5초 load는 commit 전 WAIT_TIMEOUT이었고 새 SecurityAgent 실행을 확인해 사용자의 직접 승인을 요청했다. 이는 select 동작 검사 결과가 아니다. 당시 select v1의 실행·대기 기록은 `artifacts/select-v1-validation.json`에 있다.

## 승인 후 네이티브 관찰

같은 v1에서 CLI의 Alpha→Beta와 공백을 포함한 값의 선택이 화면에 반영됐다. 합성 input·change는 순서대로 각 한 번 발생했고 같은 값 재요청은 값·이벤트와 작업 revision을 유지했다. AI 작업 중 native SELECT를 직접 누르면 해당 작업이 paused로 바뀌었고, 사용자가 Alpha를 선택하면 trusted input·change가 발생했다. 중지된 작업의 scoped read는 TASK_PAUSED로 거절돼 재개 후 결과를 읽었다. 이는 정상 보호이며 select 결함으로 세지 않는다.

이 fixture의 숨김 optgroup 자식은 Chromium native 메뉴에서 표시됐다. CLI는 숨김 옵션 메타데이터를 생략하고 선택을 거절하지만 이 정책이 native 메뉴의 표시까지 바꾸는 것은 아니다. 별도 검사에서는 ⌘⇧A와 ⌘⇧B가 AI 작업을 중지하는 공통 단축키 결함을 재현했다. 공통 Command 키 해석으로 수정한 소스는 Rust 151개와 전체 타깃 검사를 통과했다. 후속 shortcut v1에서 ⌘⇧A/B/S의 작업 상태 유지와 직접 클릭·붙여넣기의 정상 handoff를 확인했다. 결함 재현 근거는 `artifacts/select-v1-native.json`, 수정 검증 근거는 `artifacts/shortcut-v1-native.json`이다.

## 배경 탭의 포커스 계약

`artifacts/select-v1-focus-diagnostic.json`에서 숨겨진 AI 탭의 선택은 값과 합성 input/change를 바꾸지만 focus 이벤트는 즉시 실행되지 않았다. 이후 공간을 표시하자 focus 처리기가 실행됐고, 이 fixture의 처리기가 요소를 교체하면서 값이 초기화됐다. 새 전경 문서에서는 focus 중 SELECT·OPTION 교체를 모두 `ELEMENT_UNAVAILABLE`로 거절했으며 input/change와 선택값은 유지됐다. 이 보호는 명령 실행 중의 동기 검사다. 나중에 실행되는 페이지 처리기의 결과까지 고정하지 않으므로 공간을 표시한 뒤에도 필요하면 다시 읽어 확인한다. 검사 편의를 위해 사용자 공간을 자동 전환하지 않는다.

## 후속 단축키 수정 빌드

최종 검증한 shortcut v1의 SHA-256은 `e77a65a5c04efe8e596de1783d841698c34424476915622ef04e9ee0f64b22a1`이다. 승인 후 같은 바이너리의 로컬 HTTP와 네이티브 단축키·직접 입력 제어권을 확인했다. 새 바이너리에서도 선택 10개·기존 회귀 22개가 모두 통과했다. 동일 바이너리 재시작 뒤 추가 승인 없는 로컬 HTTP와 빈 탭 복원 목록을 확인했다. 검증용 작업·탭·서버를 정리하고 원래 사용자 탭 1을 보존했다. 이전 select v1 검사와는 별도로 기록한다. 근거는 `artifacts/shortcut-v1-validation.json`이다.
