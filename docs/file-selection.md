# AI 작업의 파일 선택

이 문서는 2026-10-20 파일 선택 기능의 현재 구현 계약, v20과 v21에서 관찰한 결과, 남은 E2E를 정리한다. v20은 실제 Naru와 로컬 fixture에서 파일 선택을 확인했지만 폴더 선택을 완료하지 못했다. v21에서는 폴더 요청 취소와 안내, 이후 단일 파일 선택, 요청 큐 한도와 정상 종료를 확인했다. **폴더 선택 자체는 여전히 미지원이다.** 소스 계약과 실행 결과를 구분하며 실제 변경 파일, 바이너리, 검사 결과와 정리는 [작업 기록](work-log.md)에 별도로 남긴다.

## 요청과 사용자 선택

활성 AI 작업의 페이지에서 CEF의 OPEN, OPEN_MULTIPLE 요청이 들어오면 기본 파일창을 바로 열지 않고 최대 4개의 대기 요청 안에 보관한다. 같은 탭의 중복 대기는 거절한다. 사용자는 브라우저의 파일 패널에서 요청을 확인한 뒤 선택하거나 취소한다. 요청 접수와 패널 조작만으로 사용자 공간, 선택 탭이나 AI 작업 revision을 바꾸지 않는다.

OPEN_FOLDER는 작업 소유자를 확인하기 전에 원래 CEF 콜백을 즉시 취소한다. 일반 사용자, 일시 중지된 AI 작업과 활성 AI 작업에 모두 적용하며 요청 큐나 네이티브 파일창으로 보내지 않는다. 안내 문구는 “현재 브라우저에서는 폴더 선택을 완료할 수 없습니다. 파일을 직접 선택해 주세요.”다. **폴더 선택은 현재 미지원이며 완성되지 않았다.** 이 취소 처리는 실패 경로 진입을 막기 위한 안정성 보완이다.

사용자가 파일 선택을 누를 때 네이티브 파일창을 연다. 선택 결과는 검증을 통과한 원래 CEF 파일 선택 콜백에만 전달한다. CLI로 로컬 파일 경로를 지정하거나 경로를 반환하는 기능은 추가하지 않는다. 파일 내용은 Naru의 별도 큐, 로그나 저장소에 복사하지 않는다.

파일 선택은 해당 웹페이지에 파일 접근을 허용하는 동작이다. Naru가 별도의 제출 명령을 실행하지 않아도 사이트의 `change` 처리기가 자동으로 업로드할 수 있다. 선택 전 패널에는 이 점을 짧게 안내해야 한다.

일반 사용자 탭과 일시 중지된 AI 작업의 새 OPEN, OPEN_MULTIPLE 요청은 CEF 기본 파일창을 유지한다. SAVE는 [기존 다운로드 저장 흐름](downloads-design.md)을 사용하며 AI 파일 선택 큐로 가져오지 않는다. 이미 활성 AI 상태에서 접수한 요청은 인계 뒤 일반 사용자 요청으로 재분류하지 않고 취소한다.

## 콜백과 출처의 한계

설치된 CEF 156의 `cef_dialog_handler.h`에서 `OnFileDialog`는 browser, mode, title, 기본 경로, 필터 목록과 callback을 전달한다. 요청 프레임이나 프레임 origin 인자는 없다. 따라서 브라우저의 현재 메인 문서 주소를 요청 파일 입력의 정확한 origin으로 표시하거나, 특정 iframe이 요청했다고 보증할 수 없다.

CEF 계약상 기본창을 사용할 때는 false를 반환한다. 직접 처리할 때는 true를 반환하고 원래 callback을 이어서 실행하거나 취소한다. 기본창 경로는 MIME 확장 전후 같은 요청에 대해 다시 호출될 수 있으므로 새 요청을 중복 집계하지 않아야 한다. 빈 파일 목록을 Continue에 넘기는 동작도 CEF에서는 취소로 처리한다.

Naru의 보관 대상은 원래 tab, 메인 문서 세대, 파일 요청 세대, 작업 소유자와 revision에 묶는다. CEF 필터 읽기와 콜백 보관 도중에 재진입해 프레임이 바뀐 경우도 파일 요청 세대로 거절한다. 네이티브 선택 완료 시에도 같은 대상인지 다시 확인한다. 요청 프레임을 식별할 수 없으므로 해당 탭에서 어느 프레임이든 탐색하거나 종료되면 대기 요청을 보수적으로 취소한다. 관계없는 하위 프레임 변경으로도 파일 선택을 다시 시작해야 할 수 있다.

## 취소와 네이티브 창 수명

실제 사용자 인계, 작업 revision이나 소유자 변경, 문서 이동과 탭 종료는 이전 요청을 취소한다. UI에서 파일 패널을 여는 관리 동작은 실제 사용자 인계로 처리하지 않는다.

이미 파일창을 띄운 요청이 취소되면 그 결과를 원래 페이지에 전달하지 않는다. 취소를 요청한 시점과 네이티브 창의 완료 콜백을 받은 시점은 구분한다. 완료 콜백이 오기 전까지 전역 파일창 슬롯을 점유해 두 번째 네이티브 파일창을 동시에 열지 않는다. 늦게 도착한 선택 결과는 취소된 요청을 되살리지 않아야 한다.

큐와 네이티브 창의 상태는 메모리에만 보관한다. 새 DB, 영속 파일, 별도 작업창이나 UI용 웹 renderer를 추가하지 않는다. 대기 한도 4개는 Naru가 접수한 요청의 상한이며 Chromium 내부에서 아직 전달하지 않은 대기까지 셌다는 뜻은 아니다.

[`src/file_selection.rs`](../src/file_selection.rs)의 `validate_paths`는 복수 선택에서 1개부터 64개까지, 단일 선택에서 정확히 1개 경로를 허용한다. 개별 경로는 최대 4,096 UTF-8 바이트이며 절대 경로이고 NUL을 포함하지 않아야 한다. 네이티브 결과 처리도 개수를 먼저 확인하고 경로의 UTF-16 길이를 제한한 뒤 UTF-8로 변환한다. **64개는 선택 경로의 수다.** 모델과 네이티브 코드에는 정확히 1개 경로를 받는 Folder 모드가 남아 있지만 현재 runtime이 진입을 차단하므로 지원 기능으로 간주하지 않는다. 이 개수 제한은 폴더를 지원할 때의 재귀 파일 수나 전체 바이트 크기 제한이 아니며 AppKit 내부 할당과 Chromium 폴더 열거 비용도 별도 검토가 필요하다.

[`src/file_selection_runtime.rs`](../src/file_selection_runtime.rs)의 `filter_values`는 CEF의 원본 필터 목록과 확장 필터 목록을 각각 최대 32개 읽는다. 문자열 하나는 UTF-16 코드 단위 256개를 먼저 제한하고, 변환한 UTF-8 문자열도 256바이트 이하인지 확인한다. 잘못된 UTF-16은 거절한다.

`Filters::new`는 점이 없는 형태로 정리한 확장자를 중복 없이 최대 128개 보관한다. 개별 확장자는 ASCII 64바이트 이하이며 허용 문자를 검사한다. 해석하지 못한 MIME 형식이나 잘못된 확장자가 섞이면 인식한 일부 확장자만으로 파일 선택을 좁히지 않고 확장자 목록을 비운다. 이 필터는 파일 내용이나 안전성을 판별하는 검사가 아니다.

표시용 출처는 제어 문자를 제외한 최대 80자, 형식 안내는 최대 256 UTF-8 바이트다. 선택 경로는 대기 행이나 상태 조회용 `Row`에 저장하지 않는다. 위 제한은 읽어서 확인한 모델과 runtime의 입력 경계다. 네이티브 파일창, CEF 콜백과 실제 페이지까지 통과했다는 실행 증거로 확대하지 않는다.

## v20에서 확인한 범위

v20의 [빌드 기록](../artifacts/e2e-v20-file-selection-20261020/build-receipt.json)은 소스 HEAD `836846928b153cf661b6153ab0313affac765ecb`와 빌드 중 소스가 유지됐음을 기록한다. 아래 결과는 이 빌드의 로컬 CEF 실행과 네이티브 파일창 조작 결과이며 외부 사이트 업로드 결과가 아니다.

| 항목 | 실제 관찰과 한계 |
| --- | --- |
| 단일 파일 | `sample.txt` 21바이트가 전달됐고 `input`, `change` 모두 `isTrusted: true`였다. [결과](../artifacts/e2e-v20-file-selection-20261020/single-result.json) |
| 복수 파일 | `sample.txt`, `second.md`가 각각 21바이트로 전달됐고 두 이벤트가 모두 trusted였다. [결과](../artifacts/e2e-v20-file-selection-20261020/multiple-result.json) |
| `accept=".txt"` 입력 | 실제 CUA 조작에서 네이티브 필터 제한 상태로는 `second.md`를 선택해 전달하지 못했고, Show Options에서 모든 파일 보기를 켠 뒤에 전달했다. 이 관찰은 검증 담당자가 확인했다. JSON은 변경 후 `second.md`의 전달과 trusted 이벤트만 보여주므로 필터 제한 자체의 증거로 사용하지 않는다. [전달 결과](../artifacts/e2e-v20-file-selection-20261020/filter-result.json), [조작 기록](work-log.md) |
| 상태 유지 | 위 결과에서 사용자 공간과 선택 탭은 `user`, `1`로 유지됐고 AI 작업은 active, revision 1이었다. 선택 후 파일 요청은 0개, choosing은 false였다. |
| 폴더 | 경로를 CEF에 전달한 뒤에도 폴더 입력의 이벤트가 없었다. 지연 조회에서도 같았으며 후속 chooser 요청도 접수되지 않았다. 원래 탭을 닫은 뒤 앱이 exit 139로 종료되는 현상을 재현했다. 크래시 스택이 없어 직접 원인은 확정하지 않았다. [즉시 결과](../artifacts/e2e-v20-file-selection-20261020/folder-result.json), [지연 결과](../artifacts/e2e-v20-file-selection-20261020/folder-delayed.json) |

폴더 결과에서 대기 요청이 0개라는 사실은 선택 완료를 뜻하지 않는다. 위 v20 결과로 현재 폴더 취소 변경의 동작이나 크래시 해소를 주장하지 않는다.

## v21에서 확인한 범위

v21은 위 HEAD에 `src/control_panel.rs`, `src/file_selection_runtime.rs`의 변경을 포함한 빌드다. [빌드 기록](../artifacts/e2e-v21-file-selection-20261020/build-receipt.json)과 [최종 기록](../artifacts/e2e-v21-file-selection-20261020/final.json)의 앱 SHA-256은 모두 `38026eb74a48c83faae1841fec83ad6030d7105045f8f33f4a5ec4e3bc80893f`다. 두 기록의 `sourceUnchanged`는 해당 빌드와 검증 중 소스가 유지됐다는 뜻이며 변경 없는 Git 작업 트리라는 뜻은 아니다. 아래는 이 바이너리에서 수행한 로컬 CEF와 네이티브 UI 확인 범위다.

| 항목 | 실제 관찰과 근거 |
| --- | --- |
| 활성 AI 폴더 요청 | 페이지 파일 이벤트가 없고 대기 요청 0개, choosing false였다. 사용자 공간과 선택 탭은 `user`, `1`, 작업은 active와 revision 1을 유지했다. 네이티브 AX 기록에 폴더 미지원 안내가 표시됐다. [상태](../artifacts/e2e-v21-file-selection-20261020/folder-guard.json), [안내 AX](../artifacts/e2e-v21-file-selection-20261020/01-folder-notice.ax.txt) |
| 사용자 폴더 요청 | 직접 사용 중인 사용자 탭에서도 폴더 미지원 안내와 파일 선택 요청 0개를 확인했다. [AX](../artifacts/e2e-v21-file-selection-20261020/02-human-folder-notice.ax.txt) |
| 폴더 거절 뒤 파일 선택 | 원래 AI 탭에서 `sample.txt` 21바이트를 선택했고 `input`, `change` 모두 trusted였다. 사용자 공간, 선택 탭과 작업 revision은 유지됐으며 요청과 choosing 상태가 해제됐다. [결과](../artifacts/e2e-v21-file-selection-20261020/selected-after-folder.json) |
| 큐 상한과 작업 종료 | 서로 다른 탭의 요청이 1개부터 4개까지 증가했고 다섯 번째 탭의 요청은 큐에 들어오지 않았다. 두 작업이 모두 finished가 된 뒤 AI 탭과 요청은 없어지고 사용자 탭 1개가 남았다. [결과](../artifacts/e2e-v21-file-selection-20261020/queue-limit.json) |
| 사용자 기본 파일창 | 네이티브 기본창을 열었다가 취소한 뒤 사용자 탭으로 돌아왔으며 요청은 0개, 페이지 파일 이벤트는 빈 목록이었다. [취소 후 AX](../artifacts/e2e-v21-file-selection-20261020/03-human-default-canceled.ax.txt), [최종 상태와 페이지](../artifacts/e2e-v21-file-selection-20261020/final.json) |
| 종료와 정리 | 앱은 exit 0으로 종료됐다. 검증 서버는 SIGINT로 종료했고 검증 전용 프로필 135,182,746바이트를 삭제했다. 사용자 프로필 변경은 없었다. [정리 기록](../artifacts/e2e-v21-file-selection-20261020/cleanup.json) |

v21은 폴더 거절 뒤에도 파일 선택과 탭 정리, 앱 종료가 가능한 것을 확인했다. 폴더 전달의 완성이나 v20 크래시의 직접 원인 규명으로 확대하지 않는다. 일시 중지된 AI의 폴더 요청은 소스상 같은 취소 경로지만 위 실제 실행 근거에는 포함되지 않는다.

## 폴더 경로의 제약과 남은 지원 작업

현재 고정 버전은 CEF `8db2f36735f06aacaf1bab832cb08da9d9445c55`, Chromium `156.0.8078.17`이다. Naru처럼 macOS의 외부 부모 뷰를 사용하는 Alloy 구성에서 CEF의 웹 모달 호스트는 해당 부모를 지원하지 않는다고 명시하고 있다. 또한 CEF의 `parent_view`를 지정하면 Alloy 방식이 강제된다. 이는 Naru의 일반 파일용 네이티브 패널 전체가 불가능하다는 뜻이 아니다. [CEF 모달 호스트 구현](https://github.com/chromiumembedded/cef/blob/8db2f36735f06aacaf1bab832cb08da9d9445c55/libcef/browser/alloy/dialogs/alloy_web_contents_dialog_helper.cc#L45-L61), [macOS 부모 뷰 계약](https://github.com/chromiumembedded/cef/blob/8db2f36735f06aacaf1bab832cb08da9d9445c55/include/internal/cef_types_mac.h#L150-L155)

Chromium은 업로드할 폴더의 열거가 끝나면 파일 목록을 바로 전달하지 않고 `SELECT_UPLOAD_FOLDER` 분기에서 탭 모달 확인창을 만든다. 이 경로와 위 호스트 제약은 폴더 지원에 해결이 필요한 소스 근거다. v20의 종료 현상에 대한 직접적인 크래시 스택 증거로 취급하지 않는다. [Chromium 폴더 확인 경로](https://github.com/chromium/chromium/blob/156.0.8078.17/chrome/browser/file_select_helper.cc#L243-L275)

CDP `DOM.setFileInputFiles`로 바꿔도 폴더 지원이 해결되는 것은 아니다. Blink의 `webkitdirectory` 분기는 전달받은 경로를 다시 디렉터리 열거로 보내며 Alloy에는 그 열거를 완료하는 구현이 없어 기본 취소 경로에 도달한다. Chromium 기본 `WebContentsDelegate::EnumerateDirectory`도 취소한다. CDP 호출 성공만으로 폴더 이벤트나 파일 전달이 완료됐다고 볼 수 없다. [CDP 입력 구현](https://github.com/chromium/chromium/blob/156.0.8078.17/third_party/blink/renderer/core/inspector/inspector_dom_agent.cc#L1793-L1814), [폴더 입력 분기](https://github.com/chromium/chromium/blob/156.0.8078.17/third_party/blink/renderer/core/html/forms/file_input_type.cc#L533-L542), [기본 열거 취소](https://github.com/chromium/chromium/blob/156.0.8078.17/content/public/browser/web_contents_delegate.cc#L288-L293)

폴더 지원을 완성하려면 현재 AppKit 창 구성에서 가능한 CEF 모달 호스트와 디렉터리 열거 연결을 설계하고, 원래 요청의 수명과 취소 검증을 유지한 채 실제 페이지 이벤트까지 확인해야 한다. 현재의 즉시 취소와 안내는 이 작업을 대신하지 않는다.

## 정적 검증 페이지

[`tests/fixtures/file-selection.html`](../tests/fixtures/file-selection.html)은 로컬 HTTP로 제공할 정적 페이지다. 새 서버나 Python 러너는 추가하지 않는다.

| 항목 | 입력 또는 버튼 |
| --- | --- |
| 파일 하나 | `#single`, `aria-label="파일 하나 선택"` |
| 여러 파일 | `#multiple`, `aria-label="여러 파일 선택"`, `multiple` |
| 폴더 | `#folder`, `aria-label="폴더 선택"`, `webkitdirectory`, `multiple` |
| 텍스트 필터 | `#text-only`, `aria-label="텍스트 파일 선택"`, `accept=".txt"` |
| 같은 사이트 프레임 생성 | `#add-frame` |
| 프레임 새 문서 요청 | `#navigate-frame` |
| 프레임 제거 | `#remove-frame` |

파일 입력의 `input`과 `change` 이벤트가 발생하면 `#receipt`에 입력 ID, 이벤트 종류, `isTrusted`, 파일 개수, 파일 이름과 크기를 JSON으로 표시한다. 최근 12개 이벤트만 유지하고 각 이벤트에서는 최대 64개 파일의 이름과 크기만 표본으로 표시한다. 전체 선택 개수는 별도 `count`에 남기며 생략 여부를 `truncated`로 구분한다. 이 표본 제한은 fixture의 메모리 제한이며 제품의 경로 허용 개수를 증명하지 않는다.

파일 내용, 절대 경로와 상대 경로를 읽지 않는다. form, 제출 버튼과 업로드 코드가 없으며 CSP에서 연결과 폼 전송을 막는다. 프레임은 사용자가 버튼을 누를 때만 같은 HTML의 `file-frame` query를 사용해 생성하거나 이동한다. 프레임 안에서는 재귀 프레임 제어를 숨긴다. `#frame-state`의 `navigation-requested`는 이동 명령을 내렸다는 뜻이며 새 문서 로딩 완료 증거가 아니다.

## 검증 현황과 남은 E2E

기존 8개 항목의 범위는 아래와 같다. v20과 v21의 결과는 각각 해당 바이너리의 증거로만 사용한다. 일부 단계가 확인됐다고 취소 경합이나 모든 모드가 검증된 것으로 간주하지 않는다.

1. **AI 요청 접수:** v21에서 활성 AI 요청의 대기 큐 진입과 사용자 공간, 선택 탭 유지를 확인했다. 네이티브 파일창은 사용자가 파일 패널에서 선택할 때 열었다.
2. **파일 전달과 필터:** v20에서 단일 파일, 복수 파일과 필터 조작을 확인했고 v21에서 폴더 거절 뒤 단일 파일 전달을 확인했다. 복수 선택과 필터에 대한 v20 근거를 v21의 재실행 결과로 표기하지 않는다.
3. **상태 유지와 이벤트:** v21의 폴더 거절과 후속 선택에서 작업 active, revision 1, 사용자 공간과 선택 탭 유지를 확인했다. trusted 파일 이벤트는 로컬 페이지의 전달 증거이며 외부 업로드 성공을 뜻하지 않는다.
4. **명시 취소:** v21에서 일반 사용자 기본 파일창 취소 후 이벤트가 없음을 확인했다. AI 요청 카드 취소와 AI 파일창 취소는 이 사용자 기본창 결과와 구분해 기록해야 한다.
5. **탐색과 인계 중 취소:** 네이티브 창이 열린 동안 실제 인계, 메인 문서 이동, 하위 프레임 이동과 제거, 탭 종료를 각각 구분해야 한다. v21의 작업 종료 후 탭 정리만으로 열린 파일창에 대한 모든 경합과 늦은 결과 차단을 입증하지 않는다.
6. **네이티브 완료 경계:** 취소 요청 후 완료 콜백 전에는 다음 파일창을 막고, 완료 후에는 다시 열 수 있는지 확인하는 경합 검증이 남아 있다. 큐가 최종적으로 0개인 상태만으로 중간 슬롯 수명을 입증하지 않는다.
7. **요청 한도:** v21에서 요청 4개, 다섯 번째 요청 미접수, 두 작업 종료 후 요청과 탭 해제를 확인했다.
8. **기본 동작 유지:** v21에서 일반 사용자 OPEN 기본창과 취소를 확인했다. 일시 중지된 작업, 기본 OPEN_MULTIPLE과 SAVE는 이 결과로 함께 확인됐다고 주장하지 않는다.

폴더는 위 파일 선택 성공 항목에서 제외하고 다음 두 단계로 구분한다.

- **현재 미지원 처리 검증:** v21에서 사용자와 활성 AI의 폴더 요청 거절과 안내, AI 탭의 후속 일반 파일 선택, 해당 탭 정리와 앱 정상 종료를 확인했다. 일시 중지된 AI 모드의 실제 조작 확인은 별도로 남아 있다.
- **폴더 지원의 남은 작업:** 위 CEF 연결을 구현한 뒤 폴더 열거, 확인창, trusted 이벤트, 후속 파일 선택과 탭 종료를 실제로 검증한다. 폴더 지원과 그 E2E는 현재 미완료다.

단위 검사, 정적 fixture 확인, 로컬 CEF 실행과 네이티브 파일창 조작은 각각 구분한다. 외부 사이트의 실제 업로드, 취소 경합 전체와 장시간 메모리 변화는 별도 실행 근거가 없으면 미확인으로 남긴다.
