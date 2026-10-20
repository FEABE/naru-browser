# Naru 작업 기록

## 2026-10-20, 권한 취소와 패널 포커스 보완, v24 우선 커밋

사용자가 현재 변경을 우선 커밋하고 푸시하라고 요청했다. `FEABE/naru-browser`의 PRIVATE 상태를 다시 확인했고 `codex/navigation-readiness`의 HEAD와 원격은 `dbe23b1`로 같았다. 제품 소스 두 파일과 관련 문서만 포함한다. 바이너리, 합성 프로필과 실행 자료는 Git 제외 상태로 유지한다.

`src/site_permission_runtime.rs`에서 유효한 요청의 명시적 취소는 DISMISS로, 문서와 제어권 변경 등 자동 취소는 IGNORE로 구분했다. 기존 콜백 검사에 두 결과와 만료 토큰의 중복 취소 방지를 추가했다. `src/control_panel.rs`는 사이트 권한 패널이 단독으로 열렸을 때 현재 CEF 페이지를 숨기지 않고 불투명한 AppKit 패널로 덮는다. 패널을 열 때 페이지의 입력 대상을 해제하고 닫기 버튼에 포커스를 둔다. Tab이 덮인 페이지로 이동하면 네이티브 후보를 찾고, macOS가 모든 버튼을 일반 Tab 후보에서 제외하면 현재 패널과 responder를 다시 확인한 뒤 닫기로 복귀한다. 허용 버튼을 자동 선택하지 않는다.

| 검증 구분 | 실행 결과 |
| --- | --- |
| 좁은 Rust 단위 검사 | 기존 `site_permission_callbacks_cancel_once_outside_borrows_and_forward_exact_media_mask` 1개 통과, 415개 필터 제외, exit 0. 기존 경고 4건 |
| v22 후속 로컬 CEF와 네이티브 UI | 알림 취소의 default 반환, 허용의 granted 반환과 새 문서 저장 상태, 초기화 뒤 default 확인. 배경 AI의 사용자 선택 보존, 명시적 요청 탭 표시, 인계와 재개, 문서 이동 취소 확인. 카메라와 마이크 취소의 NotAllowedError, 같은 출처 프레임 이동과 제거, 실제 대기 상한 4개와 초과 거절, 탭과 작업 종료의 대기 정리 확인 |
| v22 실패 | 보이는 페이지의 위치 카드를 취소해도 원래 조회가 끝나지 않았고 패널을 닫으면 새 토큰이 접수됨. 단순 pending 0을 취소 완료로 세지 않음 |
| v23 로컬 CEF와 네이티브 UI | 가시성 수정 뒤 보이는 사용자 페이지의 위치 취소가 error-code-1에 도달했고 패널을 닫아도 재접수되지 않음. 다른 공간에 숨긴 AI 탭은 취소 뒤 다시 표시하면 새 요청이 접수되어 미해결. Tab과 Shift-Tab 각 6회에서 덮인 CEF에 포커스가 남는 문제를 확인 |
| 통합 빌드 | v23 release 48.33초, v24 release 47.52초, 모두 exit 0. v24는 Tab 순회 실패 시 닫기 복귀 수정을 포함 |
| v24 실행 범위 | 자체 HTTP 페이지 열기와 위치 요청 접수, 사용자 공간과 선택 탭 1, pending 1, 활성 미디어 없음 확인. 사용자 요청으로 커밋을 우선해 최종 Tab, 포인터 차단과 취소 결과의 네이티브 E2E는 미실행 |
| 커밋 전 정적 확인 | `git diff --check` 통과. 제품과 빌드 파일 103개 및 앱과 CLI 해시가 빌드 이후 정리까지 동일 |
| 미실행 | 전체 회귀와 새 모의 DOM 검사는 반복하지 않음. 외부 사이트, 실제 OS 기기 허용과 미디어, IME, 장시간 메모리와 전체 14개 여정은 미완료 |

v24는 HEAD `dbe23b1`과 이번 제품 변경에서 빌드했다. 앱 SHA-256은 `d85545bfbfbc6596b2b7e6a8fac94342369b50394e40f15a594fc977a73833cc`, CLI는 `43cf93734ff8020416472b90f5ce760ac2d15e184ec4759f4adf7a2ff714caba`다. 실행 자료는 `artifacts/e2e-v22-site-permissions-followup-20261020/`, `artifacts/e2e-v23-site-permissions-20261020/`, `artifacts/e2e-v24-site-permissions-20261020/`에 보관한다. 이전 바이너리의 성공을 v24 통과로 옮기지 않는다.

`docs/site-permissions.md`에 페이지 숨김과 위치 조회의 수명, 반복 취소에 따른 Chromium 자동 제한 가능성과 숨긴 AI 탭의 남은 문제를 기록했다. `docs/file-selection.md`에는 원래 폴더 선택의 열거 경로와 별도 EnumerateDirectory 경로를 구분하고, 외부 AppKit 창의 모달 호스트 제약과 엔진 보완 후보를 남겼다. 폴더 엔진 수정이나 대규모 소스 다운로드는 진행하지 않았다. 제품 계획과 E2E 문서의 현재 후보는 v24로 갱신한다. 독립 브라우저 완성과 Preseller 내장 목표는 계속 진행 중이다.

이전 v22와 v23 앱은 각각 exit 0으로 종료했고 합성 프로필 135,203,472바이트와 135,178,447바이트를 제거했다. v24 커밋 준비 시 위치 요청은 pending 1이었다. 승인이나 버튼 취소를 추가 실행하지 않고 앱 PID 63074와 fixture 서버 PID 49760을 정상 종료해 각각 exit 0을 확인했다. 프로젝트 앱과 헬퍼, 서버는 남지 않았고 이번 합성 프로필 135,177,698바이트도 제거했다. 실제 사용자 프로필과 키체인은 변경하지 않았다.

v23 빌드는 중간 파일 3,566개, 명령 보고 기준 2.1GiB를 정리했고 v24는 1,271개, 687.8MiB를 정리했다. 최신 번들 하나를 담은 `dist` 336MiB와 release 실행 파일을 남긴 `target` 4.1MiB를 보존했다. 세 실행 자료 폴더는 각각 524KiB, 320KiB, 320KiB다. 커밋 요청 이후에는 새 빌드나 반복 회귀를 실행하지 않았다.

## 2026-10-20, 사이트 권한 구현과 v22 빌드, 우선 커밋

사용자가 진행 중인 변경을 우선 커밋하고 푸시하라고 요청했다. GitHub에서 `FEABE/naru-browser`의 PRIVATE 상태와 기존 `codex/navigation-readiness` 브랜치를 확인했다. 커밋 전 HEAD와 원격은 `8e38ebc`로 같았다. 소스, fixture와 아래 문서만 포함하며 바이너리, 프로필과 실행 자료는 Git 제외 상태로 유지한다.

`src/site_permissions.rs`에 위치, 알림, 카메라와 마이크 요청의 메모리 모델을 추가했다. 전체 최대 4개, 탭마다 1개 요청을 보관하고 토큰을 재사용하지 않는다. `src/site_permission_runtime.rs`는 원래 문서와 작업 소유권을 다시 확인한 뒤 CEF 콜백을 이어 실행하거나 취소한다. 일반 권한의 사이트 허용과 현재 요청 취소를 구분하며 정확한 origin의 위치와 알림 저장 설정만 초기화한다. 미디어 요청은 요청받은 기기 비트만 전달하고 활성 스트림 상태를 별도로 표시한다.

`src/browser.rs`에서 권한 콜백, 문서와 프레임 변경, 제어권 변경, 탭 종료를 연결했다. 제어권이 바뀐 같은 문서에서 늦게 도착한 권한 요청은 새 문서 탐색 전까지 거절한다. CLI의 `status.sitePermissions`는 상태만 조회하며 승인 명령은 추가하지 않았다. `src/control_panel.rs`에는 같은 창의 사이트 권한 패널, 요청 출처와 범위, 허용과 취소, 요청 탭 보기 및 저장 설정 초기화를 구현했다. 별도 창이나 UI renderer, DB, 상주 프로세스는 없다.

`src/lib.rs`에 모듈을 등록했고 `scripts/bundle-macos.py`에 macOS 위치 사용 설명을 추가했다. `tests/fixtures/site-permissions.html`은 버튼으로 요청하고 제한된 상태만 기록하는 자체 페이지다. 실제 알림을 발송하거나 위치 좌표와 기기 이름을 저장하지 않는다. `docs/site-permissions.md`, 제품 계획, E2E 목록과 Preseller 내장 문서에 구현 범위와 남은 검증을 반영했다.

| 검증 구분 | 실행 결과 |
| --- | --- |
| 최초 컴파일 | NSColor와 Retained 타입 불일치 2곳으로 실패. 로컬 색상 값을 참조하는 방식으로 수정 |
| 단위 검사 하네스 진단 | 컴파일 수정 뒤 reset 검사에서 exit 139. 엔진을 초기화하지 않는 검사에서 CEF 문자열 변환을 호출한 경로를 순수 UTF-16 비교로 바꿨다. 실제 앱 충돌과 구분한다 |
| 좁은 Rust 단위 검사 | `cargo test --locked --lib site_permission_`, 4개 통과, 412개 필터 제외, exit 0. 기존 불필요한 unsafe와 미사용 코드 경고 4건 |
| 통합 빌드 | `scripts/dev.sh --release --build-only`, release 컴파일 47.58초, exit 0. 기존 미사용 코드 경고 1건 |
| 정적 확인 | fixture JavaScript 구문, 번들 스크립트 Python AST와 `git diff --check` 통과. 제품과 빌드 파일 103개의 해시 및 앱과 CLI 해시는 빌드 이후 커밋 준비까지 동일 |
| 로컬 CEF | 자체 HTTP 페이지를 열고 알림 요청을 접수했다. `status.sitePermissions`에 해당 origin, 탭 1, 토큰 1과 pending 1이 표시됐다. 사용자 공간 `user`, 선택 탭 1, 활성 미디어 없음 확인 |
| 네이티브 UI | 사이트 권한 패널을 열어 요청 출처, 프로필 저장과 공간 공유 안내, 허용과 취소 및 요청 탭 보기 버튼을 확인했다. 현재 사이트의 위치와 알림은 요청 시 확인이며 초기화는 비활성이다. 패널을 열면 닫기 버튼에 포커스가 있었다 |
| 미실행 | 허용과 취소 뒤 페이지 결과, 저장 권한 초기화, 배경 AI 요청과 인계, 프레임 변경, 요청 상한의 실제 CEF 확인은 커밋을 우선해 남겼다. 전체 회귀, 새 모의 DOM, 외부 사이트, OS 기기 권한, 실제 미디어, IME와 장시간 메모리는 실행하지 않았다 |

v22는 HEAD `8e38ebc`와 이번 미커밋 제품 변경에서 빌드했다. 앱 SHA-256은 `e68b58fa0f1cbb0fef9cacefc8926faa0f0672b448dac065224b8c56bf8cd4a4`, CLI는 `43cf93734ff8020416472b90f5ce760ac2d15e184ec4759f4adf7a2ff714caba`다. 원본 로그, 해시, 상태와 화면은 `artifacts/e2e-v22-site-permissions-20261020/`에 보관한다. 권한 카드 표시를 승인 처리나 전체 09번 여정의 통과로 판정하지 않는다.

일반 권한 허용은 모든 공간이 공유하는 프로필에 저장된다. 카메라와 마이크의 사이트 승인은 macOS 권한을 대신하지 않으며 AI 중지로 이미 시작된 스트림이 끝난다고 보장하지 않는다. 제어권 변경 뒤에는 정상적인 새 요청도 문서 탐색이 필요할 수 있다. 화면 공유와 폴더 선택은 미지원이고, 독립 브라우저와 Preseller 내장 목표는 진행 중이다.

커밋 준비 직전 알림 요청은 pending 1이었다. 승인하거나 버튼 취소를 검증하지 않고 이번 앱 PID 44789를 SIGTERM으로 정상 종료해 exit 0을 확인했다. 자체 fixture 서버 PID 44305도 SIGINT 후 exit 0이었다. 앱과 헬퍼, 서버가 남지 않았으며 이번 합성 프로필 135,177,933바이트를 삭제했다. mock keychain으로 실행해 실제 프로필을 열거나 키체인 재승인을 요청하지 않았다. 빌드는 Cargo 중간 파일 4,082개, 명령 보고 기준 2.4GiB를 정리했다. 최신 번들 `dist` 336MiB와 release 실행 파일을 남긴 `target` 4.1MiB, 실행 근거 256KiB를 보존했다.

## 2026-10-20, 파일 선택 E2E와 폴더 실패 경로 보완, 커밋 준비

사용자가 현재 변경을 우선 커밋하고 푸시하라고 요청했다. GitHub에서 `FEABE/naru-browser`의 PRIVATE 상태를 확인했다. 기존 `codex/navigation-readiness`를 갱신한 결과 HEAD와 원격은 `8368469`로 같았다. 이번 범위는 아래 두 제품 파일의 수정과 파일 선택 계약, 제품 계획, E2E 목록, Preseller 내장 문서 및 작업 기록이다. 바이너리, 프로필과 실행 자료는 Git 제외 경로에 유지한다.

`src/file_selection_runtime.rs`는 OPEN_FOLDER를 작업 소유자 검사 전에 한 번 취소한다. 일반 사용자, 일시 중지된 AI와 활성 AI 모두 지원되지 않는 CEF 폴더 완료 경로로 들어가지 않도록 한다. `src/control_panel.rs`는 대기 요청이 없어도 사이드바의 **파일 선택 안내**와 기존 파일 패널에서 사유를 보여준다. 상태 조회는 공유 borrow 밖에서 한 번 수행하며 새 창이나 UI renderer, 프로필 저장을 추가하지 않았다. 폴더 지원은 미완료다.

수정 전 v20은 HEAD `8368469`에서 빌드했다. 실제 CLI와 AppKit 파일창으로 단일 파일, 복수 파일의 이름과 크기, trusted input과 change 이벤트를 확인했다. `.txt` 제한에서는 `second.md`가 선택되지 않았고 네이티브 옵션의 **모든 파일 보기**를 켠 뒤 전달됐다. 이 필터 관찰은 결과 JSON만의 판정과 구분한다. 카드와 네이티브 선택창의 취소, 열린 선택창의 handoff 취소, 같은 출처 프레임 이동과 제거에 따른 pending 취소를 확인했다. 각 검사 구간의 사용자 공간과 탭 선택도 유지됐다.

v20 폴더 선택은 실패했다. 선택 후 페이지 이벤트가 없었고 같은 탭과 새 탭의 후속 요청도 접수되지 않았다. 원래 탭 종료 후 실행 세션의 exit 139를 관찰했다. 같은 바이너리를 다시 실행한 나머지 파일 검사는 완료했고 정상 종료했다. 자료 검토의 17개 항목은 12개 성공, 5개 실패다. CLI 클릭 성공이나 pending 0을 폴더 전달 성공으로 기록하지 않는다. CEF의 외부 부모 뷰 모달 호스트 제약과 Chromium 폴더 확인 경로를 확인했으나 크래시 스택 없이 종료의 직접 원인을 단정하지 않았다.

| 검증 구분 | 실행 결과 |
| --- | --- |
| 좁은 Rust 단위 검사 | `cargo test --locked --lib file_selection_folder_cancels_once_before_context_without_queue_or_picker`, 1개 통과, 411개 필터 제외, exit 0. 취소 1회와 큐 및 선택창 미점유 확인. 불필요한 unsafe 경고 1개와 기존 미사용 코드 경고 3개 |
| 통합 빌드 | `scripts/dev.sh --release --build-only`, v20 47.95초와 v21 46.70초 모두 exit 0. 각각 기존 미사용 코드 경고 1개. 각 소스 100개의 전후 동일성 기록 |
| v21 로컬 CEF와 네이티브 UI | AI 폴더 요청 취소와 안내, 같은 탭의 후속 단일 파일 전달 및 trusted 이벤트 확인. 실제 대기 4개와 다섯 번째 요청 거절, 두 작업 finished revision 2와 대기 및 AI 탭 정리 확인 |
| v21 일반 사용자 흐름 | 사용자 탭의 폴더 안내, 이어진 CEF 기본 파일창 열기와 취소 확인. 페이지 전달 이벤트 없음. 사용자 프로필 대신 자체 합성 프로필 사용 |
| 커밋 전 정적 확인 | `git diff --check` 통과. 변경 7개 파일의 범위를 확인했고 `dist`, `target`, `.local-data`, `artifacts`는 Git 제외 상태이며 추적 파일이 없음 |
| 모의 DOM과 전체 회귀 | 반복하지 않음. 사용자 요청대로 통합 빌드 후 필요한 실제 여정 중심으로 확인 |
| 외부 사이트와 남은 경계 | 외부 업로드, OOPIF 선택, 다운로드 저장창 동시 요청, 취소 중 늦은 완료 콜백의 모든 경합, IME와 장시간 메모리는 미실행 |

v21 앱 SHA-256은 `38026eb74a48c83faae1841fec83ad6030d7105045f8f33f4a5ec4e3bc80893f`, CLI는 `5c44748e8cf604ab05b5cf2c0752793fa74757e9a783ead74b7ff07a2952c715`다. 수정 전 v20 앱은 `3f733f4bfbd1d5b1a9413292ff25f0dd99b8888e0c8104244d7a8a861a9cb8be`, CLI는 `5af462000c3c1d61b7fd314c978f2ce1c61769bb7a591cf9a7df43b6a2de3393`다. 실행 로그와 JSON, AX 기록은 `artifacts/e2e-v20-file-selection-20261020/`, `artifacts/e2e-v21-file-selection-20261020/`에 보관한다. v20의 복수 선택, 필터, 인계와 프레임 결과를 v21 통과로 합산하지 않는다.

`docs/file-selection.md`에 현재 계약과 남은 실행을, `docs/preseller-integration.md`에 모달 호스트와 디렉터리 열거 제약을 기록했다. 폴더 지원을 위해 지원되는 호스트 연결이나 엔진 보완이 필요하며 CLI 방식 변경만으로 해결됐다고 보지 않는다. `docs/product-plan.md`, `docs/ego-parity-e2e.md`의 최신 후보는 v21로 갱신한다. 09번과 전체 14개 여정, 독립 브라우저 완성과 Preseller 내장 목표는 미완료다.

v20 첫 실행 exit 139, 재실행 exit 0과 v21 exit 0을 기록했다. 자체 fixture 서버는 SIGINT로 종료해 exit 130이며 앱과 헬퍼, 서버가 남지 않았다. v20 합성 프로필 135,201,721바이트와 v21 합성 프로필 135,182,746바이트를 삭제했다. mock keychain을 사용했고 실제 사용자 프로필을 열거나 재승인을 요청하지 않았다. 빌드 정리로 v20 중간 파일 1,271개, 686.7MiB와 v21 중간 파일 3,566개, 2.1GiB를 제거했다. 단일 최신 번들 `dist` 336MiB와 release 실행 파일을 남긴 `target` 4.0MiB는 보존했다. 커밋 준비에서는 앱이나 검사를 다시 실행하지 않았으며 남은 프로젝트 프로세스가 없음을 재확인했다.

## 2026-10-20, AI 파일 선택 연결과 우선 커밋

사용자가 진행 중인 변경을 우선 커밋하고 푸시하라고 요청했다. 대상은 GitHub에서 비공개를 확인한 `FEABE/naru-browser`의 `codex/navigation-readiness` 브랜치다. 원격 갱신 시 HEAD와 원격은 `5b241c2`로 같았다. 아래 v19 대상 스크롤 실행 기록과 파일 선택 후속 소스를 함께 묶었다. 최신 앱 번들은 계속 v19이며 이번 파일 선택 구현을 포함하지 않는다.

`src/file_selection.rs`에 메모리 기반 요청 모델과 입력 상한을 추가했다. 최대 4개, 탭당 1개 요청과 재사용하지 않는 토큰으로 관리한다. `src/file_selection_runtime.rs`는 활성 AI 탭의 CEF OPEN, OPEN_MULTIPLE, OPEN_FOLDER 콜백을 보관하고 사용자 선택 뒤 원래 콜백으로만 전달한다. `src/browser.rs`에서 제어권, 문서와 파일 요청 세대를 확인하며 요청 접수 전 재진입과 선택 완료 준비 중 취소도 다시 검사한다. 인계, 탐색, 프레임 종료, 탭 종료와 렌더러 실패는 기존 요청을 취소한다. 일반 사용자와 일시 중지된 작업은 CEF 기본 선택창을 유지한다.

`src/control_panel.rs`의 기존 파일 패널에 요청 카드, 선택과 취소 버튼, 사이드바 요청 수를 연결했다. `src/file_selection_native.rs`는 사용자가 누를 때 비동기 NSOpenPanel 시트를 한 개 열고 필터 및 모든 파일 보기를 지원한다. 선택한 파일은 사이트에서 바로 업로드할 수 있다는 안내를 표시한다. 콜백은 공유 상태를 빌린 구간 밖에서 처리하며 취소된 시트는 실제 완료까지 슬롯을 유지한다. `src/download_runtime.rs`에는 다운로드 저장창과 파일 선택창의 중복 개방 방지를 연결했다.

새 경로 지정 CLI, DB나 영속 저장소, UI용 renderer는 추가하지 않았다. `status.fileSelection`에는 대기 상태와 요청 식별자만 포함한다. `src/lib.rs`, `Cargo.toml`, `Cargo.lock`에는 모듈과 기존 버전의 block2 및 필요한 AppKit 기능을 연결했다. `tests/fixtures/file-selection.html`은 자체 파일의 이벤트와 제한된 이름, 크기만 표시하는 로컬 검증 페이지다. `docs/file-selection.md`, 제품 계획과 E2E 체크리스트에 계약과 미실행 여정을 기록했다.

| 검증 구분 | 이번 실행 결과 |
| --- | --- |
| 최초 컴파일 | `error[E0308]: mismatched types`, NSOpenPanel의 `setTitle`이 `Option<&NSString>`를 요구해 실패. `Some`으로 감싸 수정 |
| Rust 단위 검사 | `cargo test --locked --lib file_selection_`, 신규 8개 통과, 403개 필터 제외. 테스트 코드의 불필요한 unsafe 경고 1개와 기존 미사용 코드 경고 3개. exit 0 |
| 실제 실행 경로 컴파일 | `cargo check --locked --bins`, exit 0. 기존 미사용 코드 경고 1개 |
| 정적 검증 | fixture의 inline JavaScript에 `node --check`, `git diff --check` 통과. 변경 범위에 바이너리, 프로필과 실행 자료가 없는지 확인 |
| 모의 DOM, 로컬 CEF, 네이티브 UI, 외부 사이트 | 이번 파일 선택 변경에서는 미실행. 사용자 요청에 따라 커밋과 푸시를 우선하고 통합 빌드 후 E2E로 남김 |

파일 선택 상한은 선택 경로 수와 문자열 크기의 제한이다. 폴더 안에서 Chromium이 열거하는 파일 수와 전체 메모리 상한은 아니다. CEF 파일 요청에는 정확한 프레임 출처가 없어 다른 하위 프레임의 이동도 해당 탭의 요청을 취소한다. 실제 요청 카드에서 파일 전달, 취소 후 재선택, 다운로드 저장창 공존과 장시간 메모리는 아직 확인하지 않았다. 단위 검사 통과를 ego lite 수준의 제품 완성이나 실제 업로드 성공으로 판정하지 않는다.

이번에는 앱과 HTTP 서버를 실행하지 않았고 Naru 앱, 헬퍼 프로세스도 남아 있지 않았다. 검사 뒤 Cargo 중간 파일 3,054개, 명령 보고 기준 2.0GiB를 정리했다. 기존 release 실행 파일을 남긴 `target` 3.9MiB와 v19 번들 `dist` 336MiB는 보존했다. 검사 로그는 Git 제외 경로 `artifacts/commit-file-selection-20261020/`에 남겼다. 사용자 프로필은 변경하지 않았다.

## 2026-10-20, v19 대상 스크롤 통합 빌드와 실제 CEF 확인

직전 사용자 요청으로 대상 스크롤 소스를 `5b241c2`에 커밋하고 비공개 원격에 푸시했다. goal을 재개해 깨끗한 같은 소스에서 통합 앱을 한 번 빌드하고 준비한 로컬 CEF 여정을 실행했다. 이번 실행에서는 제품 코드와 검사 코드를 추가로 수정하지 않았다.

`scripts/dev.sh --release --build-only`는 exit 0, release 컴파일 44.33초였다. 기존 미사용 코드 경고 1건이 있었으며 앱 SHA-256은 `c91eda4a06875f6fec64b9f42f902ada8bca0abb2f68bcf6123840fc5deafa14`, CLI는 `f1acc03709aa5556e43246c641d582ff60423c00a787c30fc8b66b1684f233da`다. 제품과 빌드 관련 파일 97개의 해시, Git revision과 바이너리 해시를 기록했고 실행 뒤에도 같았다. 새 격리 프로필 `.local-data/e2e-v19-targeted-scroll-20261020`과 기존 mock keychain 모드로 실행해 실제 사용자 프로필을 열거나 키체인 재승인을 요청하지 않았다.

`tests/scroll_target_smoke.py`는 로컬 HTTP 서버 두 개와 실제 Naru CLI 80회로 81항목을 확인했고 exit 0이었다. 메인 목록, 열린 Shadow DOM 목록, 일반 `read`에 포함된 동일 출처 프레임 목록에서 x 43, y 61의 상대 이동을 확인했다. 동일 출처와 별도 child session이 있는 OOPIF 문서는 y 61로 이동했다. OOPIF 목록도 x 43, y 61로 이동했고 다음 읽기의 좌표가 응답과 일치했다. 각 여정에서 논리적 DOM 포커스와 다른 목록 및 부모 문서의 좌표가 유지됐다. 사용자 공간 `user`, 선택 탭 1, 닫힌 공간 개요 상태도 시작과 끝에 같았다.

같은 `read-page` 결과의 서로 다른 문서 참조를 연속 사용했다. 새 읽기 전의 참조, handoff 전 revision, paused 작업과 resume 뒤 옛 프레임 참조는 거절됐다. 요소를 다른 문서로 옮긴 뒤 옛 참조로 이동하지 않았고, 기존 `scroll TAB DX DY`의 메인 문서 이동도 유지됐다. 관찰한 좌표와 DOM 호출 계약의 확인이며 실제 휠 이벤트나 비동기 목록 로딩 완료의 증거는 아니다.

| 검증 구분 | 이번 실행 결과 |
| --- | --- |
| 실제 CEF, 로컬 HTTP | 위 81항목 통과. 원본 응답과 확인 목록은 `artifacts/e2e-v19-targeted-scroll-20261020/scroll-run-1.json` |
| 최종 상태 | 사용자 빈 탭 1개, 검사 작업 finished revision 4. focus lease, 캡처, 탭 생성 예약, wait와 다운로드 대기 모두 0 |
| 단위 검사와 모의 DOM | 이번 실행에서는 반복하지 않았다. 직전 같은 소스의 신규 6개와 9개 통과는 앞선 기록을 따름 |
| 네이티브 UI와 외부 사이트 | 미실행. AppKit 포커스, IME, 실제 로그인과 외부 가상 목록의 동작까지 확인한 결과가 아님 |
| 메모리와 지연 | 장시간 사용, 큰 DOM의 읽기 지연과 RAM은 측정하지 않음. 자료 크기나 대기 자원 0을 메모리 성능으로 환산하지 않음 |

읽기 검토에서는 기존 순회와 참조 개수 한도를 유지하고 새 전역 캐시나 타이머가 없음을 확인했다. 다만 일반 `read`의 기존 강한 요소 참조와 추가된 body 참조는 다음 읽기까지 교체된 DOM 하위 트리를 보관할 수 있다. 최대 참조 개수는 보관된 DOM의 바이트 상한이 아니다. 스크롤 대상을 찾기 위해 노드별 레이아웃 수치를 더 읽으므로 큰 문서에서의 지연도 후속 측정이 필요하다. 이 검토를 누수 재현이나 성능 개선의 실측으로 기록하지 않는다.

`docs/product-plan.md`, `docs/ego-parity-e2e.md`에 v19의 범위와 다음 미완료 항목을 반영했다. 전체 14개 여정의 통과나 ego lite와의 전체 동등성, Preseller 내장 완료를 선언하지 않는다. AI 파일 선택 연결, 다운로드 외 사이트 권한, 기존 네이티브 경계와 장시간 사용 등 남은 범위는 계속 개발 대상이다.

러너가 자체 작업 탭과 서버 두 개를 종료한 뒤 상태를 저장했다. 이번 앱 PID 3164에 SIGTERM을 보내 exit 0으로 끝냈고 제어 소켓과 프로젝트 앱, 헬퍼, 러너 프로세스가 남지 않았다. 새 합성 프로필 12,613,509바이트를 정리했다. 빌드 중간 파일 1,271개, 명령 보고 기준 684.6MiB도 제거됐으며 최신 번들 336MiB와 release 실행 파일을 남긴 `target` 3.9MiB를 보존했다. 근거는 Git 제외 경로 `artifacts/e2e-v19-targeted-scroll-20261020/`의 116KiB다. 사용자 데이터는 변경하지 않았다. 이번 후속 문서 변경은 커밋하거나 푸시하지 않았다.

## 2026-10-20, 대상 스크롤 구현과 우선 커밋

사용자가 진행 중인 변경을 우선 커밋하고 푸시하라고 요청했다. `FEABE/naru-browser`의 비공개 상태를 GitHub에서 확인했고 기존 `codex/navigation-readiness` 브랜치를 사용한다. 원격 갱신 후 HEAD와 원격은 `9d32ee4`로 같았다. 아래 v18 다운로드 E2E 기록과 이번 대상 스크롤 소스를 함께 반영한다.

`src/cli.rs`, `src/protocol.rs`에 `scroll TAB REF DX DY`와 `scroll_element` 명령을 추가했다. 기존 페이지 스크롤 형식은 유지하고 참조 길이, 탭과 이동량, 작업 범위를 검사한다. `src/browser.rs`는 메인 문서와 프레임 참조의 기존 라우팅에 연결하고 실행 직전 문서, 취소와 제어권을 다시 확인한다. 이 명령에 포커스 준비나 공간 전환을 추가하지 않았다.

`src/dom_bridge.js`는 기존 읽기 예산 안에서 목록과 문서의 스크롤 좌표, 크기와 지원 축을 반환한다. 참조는 읽을 때의 스크롤 요소에 묶이며 DOM `scrollBy` 한 번과 동기 좌표 관찰로 끝낸다. 조상 이동이나 네이티브 휠 입력을 추가하지 않는다. 새 읽기의 오래된 참조와 원래 문서에서 이동한 요소를 거절하고, 호출 뒤 결과가 불확실하면 `attempted`와 `outcomeUnknown`을 전달한다. 추가 요소 연결에는 WeakRef를 사용한다.

`tests/scroll_target_smoke.py`, `tests/fixtures/scroll-target.html`에 메인 목록, 열린 Shadow DOM, 동일 출처 프레임과 실제 OOPIF, 문서 이동과 제어권 변경을 위한 로컬 실행 목록을 준비했다. 기존 `tests/frame_journey_smoke.py`의 지원 행동 기대값도 scroll을 포함한 8개 이름으로 갱신했다. `README.md`, `docs/targeted-scroll.md`, 프레임 계약, 제품 계획과 E2E 체크리스트에 구현과 미실행 범위를 구분했다.

검증 결과는 다음과 같다.

- `cargo test --locked --lib scroll_element_`: 신규 Rust 단위 검사 6개 통과, 나머지 397개는 필터 제외. 기존 미사용 코드 경고 3건이 있었고 종료 코드는 0이다.
- `node --test --test-name-pattern='referenced scroll' tests/dom_bridge.test.mjs`: 모의 DOM 검사 9개 통과, 128개 제외. 실제 CEF 결과가 아니다.
- 새 러너와 수정한 프레임 러너의 Python AST, fixture의 inline JavaScript 구문, `git diff --check`를 확인했다. 읽기 검토에서 CLI, 프로토콜, 문서별 실행 경로의 연결을 확인했다.
- 사용자 요청에 따라 새 앱 번들, 로컬 CEF 통합, 네이티브 UI와 외부 사이트 검증은 커밋 뒤로 남겼다. 전체 회귀 묶음을 실행하지 않았다. 아래 v18의 다운로드 검증은 이 스크롤 구현의 성공 근거가 아니다.

문서의 body 참조는 문서 viewport를 대표하므로 body 자체의 독립 overflow 목록을 별도 대상으로 표현하지 않는다. DOM의 동기 이동은 후속 가상 목록 로딩이나 네이티브 휠 이벤트를 보장하지 않는다. 지원 축, 실제 위치, 포커스와 사용자 선택 보존은 다음 통합 빌드의 E2E에서 확인해야 한다. 독립 브라우저와 Preseller 내장 목표는 진행 중이다.

이번 커밋 준비에서는 앱과 HTTP 서버를 실행하지 않았고 남아 있는 프로젝트 앱이나 검사 프로세스도 없었다. Cargo 중간 파일 2,299개, 명령 보고 기준 1.4GiB를 정리하고 기존 release 실행 파일을 보존했다. `target`은 3.9MiB, 단일 최신 앱이 있는 `dist`는 336MiB다. 검사 로그는 Git 제외 경로 `artifacts/e2e-v19-targeted-scroll-20261020/`에 남겼다. 바이너리, 프로필과 실행 자료를 커밋에 포함하지 않으며 사용자 데이터는 변경하지 않았다.

## 2026-10-20, v18 통합 빌드와 다운로드 재요청 E2E

직전 요청의 커밋 `9d32ee4`와 비공개 원격 푸시는 완료된 진전이다. goal을 재개해 같은 깨끗한 소스에서 release 앱을 한 번 빌드하고, 아직 확인하지 않았던 수동 다시 받기 사용자 여정을 실제 AppKit UI와 로컬 CEF에서 확인했다. 제품 코드는 추가 수정하지 않았다.

`scripts/dev.sh --release --build-only`는 exit 0이었다. release 컴파일 44.90초와 기존 dead_code 경고 1건을 기록했다. 앱 SHA-256은 `0d8d2c5bd1db019768c2eda745b0e78d7177cb6fad90e777aac40a8e165fa450`, CLI는 `79e398f1a54b8254579d916adc242e1308964a596570d85f939f621bde0d0607`이며 실행 전후 제품 소스 manifest와 바이너리가 같았다. 별도 검증 프로필과 기존 mock keychain 모드로 실행해 반복 키체인 승인을 요청하지 않았다.

실패 응답의 내부 재시도 6회 뒤 중단 UI를 확인하고 서버를 복구했다. 실제 다시 받기 버튼은 새 ID와 저장창으로 이어졌으며 이전 행의 버튼은 사라졌다. 새 저장창을 취소한 뒤 해당 취소 행에서 다시 요청하고 새 이름으로 저장했다. 파일은 131,072바이트와 fixture 원본 SHA-256이 일치했고 부분 파일은 없었다. 배경 AI 탭에서도 취소 후 재요청은 사용자 공간과 선택 탭 1, AI active revision 1을 유지했다. handoff revision 2는 새 저장 대기를 취소했으며 최종 작업은 finished revision 3, 다운로드와 저장 및 권한 대기는 모두 0이었다.

전체 회귀 묶음과 단위 검사를 다시 실행하지 않았다. 별도 동적 포트의 독립 fixture HTTP 검사 19개는 제품 앱 검증과 구분한다. 모의 DOM, 외부 사이트, IME, 장시간 메모리, Finder의 실제 선택, 응답 없는 재요청 timeout과 탐색 초기 거절 안내는 미실행이다. 저장창에서 최신 AX 조회 뒤 옛 ID를 쓴 한 번의 도구 거절은 최신 ID로 해결했다. 앱 로그의 저장 전 파일 경로 조회 경고도 보존했다.

`docs/native-e2e-2026-10-20.md`, `docs/ego-parity-e2e.md`, `docs/downloads-design.md`, `docs/product-plan.md`에 같은 바이너리의 확인 범위와 남은 항목을 반영했다. 읽기 검토에서 메인 창만 조작하는 현재 CLI 스크롤과 별도 목록 및 프레임 대상 스크롤의 차이를 확인해 다음 구현 후보로 기록했다. AI와 사용자 파일 선택 연결, 다운로드 외 권한 안내는 기능 누락이고 IME나 긴 탭 목록 조작은 기존 구현의 검증 범위라는 차이도 남겼다. 전체 독립 브라우저와 Preseller 내장 목표는 완료되지 않았다.

앱과 두 fixture 서버는 모두 exit 0으로 종료했다. 검증 작업과 탭을 정리했고 프로젝트 앱 프로세스, 제어 소켓과 18765 포트가 남지 않았다. 이번 합성 프로필의 Chromium 데이터 135,205,025바이트와 빌드 중간 파일 1,271개, 명령 보고 기준 684.4MiB를 정리했다. 단일 최신 번들 336MiB, release 실행 파일을 남긴 target 3.9MiB와 Git 제외 증거 `artifacts/e2e-v18-download-restart-20261020/`만 보존했다. 사용자 프로필과 다른 프로젝트는 건드리지 않았다. 이번 후속 문서 변경은 커밋하거나 푸시하지 않았다.

## 2026-10-20, 수동 다시 받기 구현과 우선 커밋

사용자가 진행 중인 변경을 우선 커밋하고 푸시하라고 요청했다. 대상은 GitHub에서 비공개를 확인한 `FEABE/naru-browser`의 기존 `codex/navigation-readiness` 브랜치다. 원격 갱신 시 HEAD `c40bf03`과 원격은 같았다. 새 통합 앱 빌드보다 현재 소스와 검증 범위의 기록을 먼저 반영한다.

`src/downloads.rs`, `src/download_runtime.rs`, `src/browser.rs`, `src/control_panel.rs`에 취소되거나 실패한 파일의 **다시 받기**를 연결했다. 유일한 HTTP(S) GET 증거가 있는 요청만 새 다운로드 ID와 새 저장 위치 선택으로 이어진다. 원본 탭, 문서와 제어권을 재검증하고 이전 경로를 저장 승인으로 재사용하지 않는다. 접수 전 예약 하나도 활성 한도 4개에 포함하며 새 항목이 접수되면 원본 항목의 재요청 URL을 소비한다. URL은 항목별 8,192바이트까지 메모리에 보관하며 목록과 후보는 각각 24개로 제한한다. 이전 작업의 탐색 거절 안내 수정도 함께 포함한다.

`tests/download_fixture.py`와 `tests/fixtures/downloads.html`에는 명시적으로 복구하기 전까지 계속 불완전 응답을 보내는 `/recoverable.bin`을 추가했다. localhost `POST /recover`에 정확한 `{"ready":true}`를 전달한 뒤에는 같은 주소에서 정상 파일을 받는 구조다. `docs/downloads-design.md`, `docs/product-plan.md`, `docs/ego-parity-e2e.md`에 소스 구현과 실제 앱 미검증을 구분했다. 이전 v17 파일 UI 실행의 근거는 바로 아래 기록과 `docs/native-e2e-2026-10-20.md`에 유지한다.

검증 결과는 다음과 같다.

- `cargo test --locked --lib manual_restart_`: 신규 단위 검사 4개 통과, 실패 0, 나머지 393개는 필터로 제외. URL과 상태 제한, 유일한 GET과 문서 일치, 예약 중복 방지와 활성 한도, 문서별 provenance 무효화를 확인했다. 기존 미사용 코드 경고 3건이 있었으며 종료 코드는 0이다.
- 먼저 실행한 같은 필터의 `--bin light-browser`는 검사 0개였다. 컴파일만 확인한 결과이며 단위 검사 통과 수에 포함하지 않는다.
- 수정한 UI와 다운로드 Rust 파일의 format 검사, `git diff --check`, fixture Python AST 구문 확인을 통과했다.
- 모의 DOM, 전체 회귀 묶음, 새 앱 번들, 이번 구현의 로컬 CEF 통합과 네이티브 UI, 외부 사이트 검증은 실행하지 않았다. 현재 설치된 v17 실행 결과를 이 구현의 성공 증거로 사용하지 않는다.

다음 통합 후보에서는 실패 표시 후 서버 복구, 다시 받기, 새 저장창 취소 후 다시 받기, 실제 파일 해시, 사용자 공간과 탭 선택 보존을 확인해야 한다. `OnBeforeDownload` 없이 실패하면 30초 뒤 안내하고 원본 탭을 닫아 예약을 정리해야 한다. 같은 탭에 다른 다운로드 후보가 남아 있어도 재요청을 보수적으로 제한한다. POST, blob, data와 출처가 모호한 다운로드는 원본 페이지에서 다시 받아야 한다. Finder의 실제 파일 선택, IME, 지속 사용과 실제 로그인 검증은 기존 미완료 범위다.

이번 커밋 준비에서는 앱이나 fixture 서버를 실행하지 않았고, 프로세스 목록에서 해당 프로젝트 앱과 다운로드 fixture 서버가 남아 있지 않은 것을 확인했다. 단위 검사 후 `scripts/cleanup-build.py`로 Cargo 중간 파일 2,844개, 명령 보고 기준 1.7GiB를 제거했다. 기존 release 실행 파일을 보존했으며 `target`은 3.9MiB다. 검사 로그는 Git 제외 경로 `artifacts/e2e-v18-download-restart-20261020/`에 남겼다. `dist`, `target`, `.local-data`, `artifacts`가 Git에서 제외됨을 확인했으며 사용자 프로필, 바이너리와 다운로드 파일은 커밋에 포함하지 않는다.

## 2026-10-20, 파일 UI 후속 확인과 탐색 거절 안내

직전 요청의 커밋 `c40bf03`과 비공개 원격 반영은 완료됐다. 이번 goal 재개는 깨끗한 같은 checkout에서 시작했다. Naru 완성을 위한 후속 작업으로 기존 v17의 파일 저장과 취소를 확인하고, 로딩 중지 조사에서 드러난 초기 거절 안내 누락을 수정했다.

`src/control_panel.rs`의 `navigation_control_action`에서 오류 안내를 주소 초안 갱신 receipt와 분리했다. 같은 호출, 창과 컨트롤, 탭, 공간, URL이 유지되고 오버레이가 없으면 초기 epoch 검사에서 거절한 이유도 기존 상태줄에 표시한다. 주소 초안의 갱신 조건, 실제 명령 전송의 epoch와 소유권 검사, 중지 의도는 유지하며 자동 재전송은 추가하지 않았다. 과거 첫 중지 실패의 원인으로 이 경로를 확정한 것은 아니다. 당시 자료에는 action 도달과 epoch가 없고, 같은 `/slow`의 추가 요청도 없어 Stop이 Reload로 바뀌었다는 근거도 없다.

소스 검증은 `rustfmt --edition 2024 --check`, `git diff --check`, `CEF_PATH="$PWD/.cache/cef" PATH="$PWD/.cache/bin:$PATH" cargo check --locked --bins`를 통과했다. 컴파일 확인은 28.71초, exit 0이며 기존 `Registry::remove`, `len`, `is_empty`의 dead_code 경고 1개가 있었다. 단위 테스트와 모의 DOM 검사, 전체 회귀 묶음은 실행하지 않았다. 새 앱 번들을 만들지 않아 수정된 거절 안내의 네이티브 표시와 첫 중지 경합 재현은 다음 통합 후보에서 확인해야 한다.

파일 UI는 변경 전 v17 앱 해시 `909809680609d3fc847714f3be961bb0a3aa795d36af937ab7296ad1233e7d98`, CLI 해시 `da321cbfe2d47b48836c21fbaf9f89750c7e69cfc5efb23642c01b6f0a58a784`로 확인했다. 별도 합성 프로필 `.local-data/e2e-v17-files-20261020`과 기존 mock keychain 옵션, localhost `tests/download_fixture.py`를 사용했다. 새 폴더의 초기 권한 755는 `INSECURE_SOCKET_DIRECTORY`로 실행이 거절됐고, 이 검증 폴더만 700으로 설정해 실행했다. 화면 좌표로 페이지 링크를 누른 첫 시도는 요청이 없어 성공으로 계산하지 않았고, 주소창에서 attachment 주소를 입력해 파일 요청을 시작했다.

- 네이티브 저장창 Cancel은 취소됨으로 끝났고 저장 대기는 0이었다.
- 느린 파일을 저장하고 진행 중 취소했다. 화면의 40% 진행에서 취소됨으로 바뀌었으며 서버 연결 해제 1건, 활성 연결 0, 앱 다운로드와 저장 및 권한 대기 0, 저장 폴더의 부분 파일 0을 확인했다.
- 불완전 응답은 서버 요청 6회 뒤 중단 코드 38로 표시됐다. 별도 저장창을 반복 요구하지 않았고 부분 파일은 남지 않았다. 이는 Chromium 내부 재시도 결과다. 사용자가 중단 항목에서 다시 받는 UI 버튼은 현재 구현돼 있지 않다.
- 이후 정상 파일을 새 요청으로 저장해 131,072바이트와 fixture 원본 SHA-256 `59f410ae5e17962412e2aed4f815918f634932f2abf084f00bb638c4db017850`의 일치를 확인했다.
- Finder 버튼 뒤 Naru의 요청 안내는 확인했다. Finder UI 조회가 2,458.67초 지연된 뒤 최근 항목 화면을 반환해 대상 파일 선택은 확인하지 못했다. 이 지연의 원인과 실제 Finder 선택 결과는 미확정이며, 관련 없는 파일 목록은 실행 자료에 저장하지 않았다.

근거는 Git 제외 경로 `artifacts/e2e-v17-files-20261020/`의 AX 자료와 `09-cancel-receipt.json`, `12-file-receipt.json`, `source-check.log`, `final.json`이다. `docs/native-e2e-2026-10-20.md`, `docs/ego-parity-e2e.md`, `docs/product-plan.md`에 확인한 범위와 수동 다운로드 재요청의 미구현 상태를 반영했다. 외부 사이트나 실제 계정, IME, 장시간 반복 여정은 실행하지 않았다. Finder 조회 대기로 앱이 켜져 있던 시간을 지속 사용 검사로 계산하지 않는다.

정리에서는 이번 앱과 fixture 서버를 종료 코드 0으로 끝내고 프로젝트 앱 프로세스와 제어 소켓이 남지 않은 것을 확인했다. 이번 합성 프로필의 Chromium 데이터 135,204,528바이트를 정리하고 방문 기록 JSON 138바이트와 실행 근거, 검증 파일만 남겼다. `scripts/cleanup-build.py`로 Cargo 중간 파일 3,639개, 명령 보고 기준 2.3GiB를 제거하고 기존 release 실행 파일은 보존했다. 앱과 CLI 해시는 바뀌지 않았으며 실제 사용자 프로필과 다른 검증 자료는 건드리지 않았다. 이번 변경은 커밋하거나 푸시하지 않았다.

## 2026-10-20, 현재 변경 커밋과 비공개 원격 반영

사용자가 현재 작업의 커밋과 푸시를 요청했다. `FEABE/naru-browser`가 비공개 저장소임을 GitHub에서 확인했고, 대상은 기본 브랜치가 아닌 `codex/navigation-readiness`다. 원격 갱신 후 로컬에만 있는 기존 커밋 7개와 새 변경을 함께 반영하는 범위다. 이번 작업에서 제품 코드를 추가로 수정하지 않았다.

커밋에는 `src/browser.rs`, `src/frame_sessions.rs`, `src/mac.rs`, `src/main.rs`의 프레임 렌더러 종료 처리, DevTools 콜백 처리 지연, 정상 종료 보완과 관련 검사 자료를 포함한다. 다운로드 권한 검사 수정, v15부터 v17까지의 E2E 러너, 프레임 fixture, 네이티브 E2E 결과와 제품 문서도 포함한다. 버전별 러너는 실행 전에 해당 `artifacts/e2e-vXX/cli` 출력 폴더를 만들어야 한다.

커밋 준비 과정에서 `git diff --check`와 변경된 Python 파일 20개의 AST 구문 확인을 통과했다. 구문 확인은 테스트 실행 결과가 아니다. 단위 테스트, 모의 DOM 검사, 로컬 CEF 통합 검사, 네이티브 UI 확인, 외부 사이트 검증과 빌드는 이번 요청에서 반복하지 않았다. 이전 실행의 결과와 미확인 범위는 아래 기록과 `docs/native-e2e-2026-10-20.md`에 유지한다. 첫 로딩 중지 원인, 남은 파일 UI 흐름, IME, v17 지속 사용과 실제 로그인 등은 여전히 미완료다.

`dist`, `target`, `.local-data`, `artifacts`는 Git 제외를 확인했다. 앱 바이너리, 사용자 프로필, 실행 로그와 다운로드한 검증 파일은 커밋 대상이 아니다. 이번에는 앱이나 서버를 실행하지 않았고 새 빌드 산출물도 만들지 않아 추가 정리 대상은 없다. 이전 네이티브 검증의 프로세스 종료와 프로필 정리 결과는 바로 아래에 기록돼 있다.

## 2026-10-20, 기존 v17의 네이티브 사용 흐름 확인

직전 커밋과 푸시는 완료된 진전이었다. 이번 goal 재개 시 실제 checkout은 HEAD `607fd69`와 기존 v17 미커밋 변경 상태여서 이전 v10 상태를 기준으로 구현을 반복하지 않았다. 제품 소스와 기존 검사 자료를 보존하고, 아직 확인하지 못한 네이티브 UI 여정을 기존 v17 앱으로 실행했다. Naru 독립 브라우저 완성과 이후 Preseller 내장 목표는 진행 중이다.

Codex Computer Use에서 AppKit 접근과 입력이 가능했다. 앞선 Antigravity의 접근성 권한 문제를 이 환경의 차단 사유로 적용하지 않았다. 앱 해시 `909809680609d3fc847714f3be961bb0a3aa795d36af937ab7296ad1233e7d98`, CLI 해시 `da321cbfe2d47b48836c21fbaf9f89750c7e69cfc5efb23642c01b6f0a58a784`는 기존 v17 기록과 같았다. 별도 `.local-data/e2e-v17-native-20261020`과 이미 구현된 `NARU_MOCK_KEYCHAIN=1`을 사용했다. 키체인 재승인이나 시스템 보안 설정 변경은 요청하지 않았다. 실제 사용자 프로필의 로그인과 안정 서명 확인은 아니다.

네이티브 UI에서 중앙 주소 입력, 주소창 이동, 뒤로와 앞으로, 잘못된 주소 보존, 두 번째 지연 중지, 북마크 추가와 이름 수정, 검색과 열기, 탭 생성과 닫기 및 재열기, 두 행 드래그의 선택 보존을 확인했다. 통합 검색을 왕복해도 한글 주소 초안을 유지했다. 배경 AI A와 B를 만들어도 사용자 공간과 초안은 바뀌지 않았다. 공간 카드의 중지는 A만 paused revision 2로 바꿨고, 재개 뒤 본문 직접 클릭과 입력은 revision 4로 인계했다. 읽은 값은 `human-input`이었다. 다시 재개한 뒤 클릭 없는 키 입력은 revision 6으로 인계했으며 B는 active revision 1과 읽기를 유지했다. 이전 revision은 `TASK_STALE_REVISION`으로 거절했다.

실제 다운로드 저장창으로 검증 폴더에 131,072바이트 파일을 저장했고 fixture 원본 SHA-256과 일치했다. 파일 선택창으로 같은 파일을 골라 localhost 페이지에 파일명과 크기, trusted 이벤트 표시를 확인했다. 외부 업로드는 없다. A와 B를 finish한 뒤 정상 종료하고 같은 바이너리로 재실행했으며 UI에서 만든 한글 북마크의 유지와 열기를 확인했다. AI 전용 `/a`는 검증 방문 기록에 없었다. `/b`는 사용자가 먼저 방문한 주소라 AI 기록 제외의 독립 근거로 쓰지 않았다.

제한과 실패 관찰도 남겼다. 첫 중지 시도는 클릭 뒤 로딩이 계속됐고 새 검색 요청이 `NAVIGATION_BUSY`로 거절된 뒤 느린 페이지가 완료됐다. 정확한 클릭 시점과 원인은 미확정이다. 중지 표시 확인과 클릭을 같은 호출로 연결한 두 번째 시도는 중지됐고 해당 경로 요청은 1회였다. Google 검색 URL 변환은 확인했지만 결과는 `/sorry/` 접근 제한으로 blocked다. 한글 typeText는 전체 문구를 입력하지 못해 붙여넣기를 사용했으며 IME 통과나 제품 결함으로 판단하지 않았다. 저장창 경로 붙여넣기도 반영되지 않아 실제 입력 필드의 setValue를 사용했다. 공간 카드 갱신 뒤 오래된 접근성 ID를 사용한 한 번의 도구 거절은 새 ID로 해결했다. 종료 로그에는 GPU adapter와 저장 패널의 파일 경로 조회 경고가 있어 빈 stderr라고 기록하지 않는다.

단위 검사, 모의 DOM, 전체 CEF 회귀 묶음과 새 빌드는 실행하지 않았다. 이번 근거는 네이티브 UI, localhost fixture와 CLI 대조, 제한된 외부 검색 탐색이다. 실행 중 제품 소스와 앱 및 CLI 해시가 바뀌지 않았음을 확인했다. 과거 v17 빌드 시점 전체 소스 manifest는 찾지 못했으므로 현재 소스 전체의 빌드 대응 관계를 새로 입증한 결과로 쓰지 않는다.

변경 문서는 `docs/native-e2e-2026-10-20.md`, `docs/ego-parity-e2e.md`, `docs/product-plan.md`와 이 기록이다. 실행 자료는 `artifacts/e2e-v17-native-20261020/`에 있으며 Git 제외 대상이다. 새 제품 기능이나 코드 수정은 없다. 북마크와 기록 삭제, 긴 목록 가장자리 이동, 입력 중 UI 인계 경합, 휠, 파일 권한과 재시도, Finder, 실제 IME, 최소 창, 다크 모드, v17 지속 사용과 실제 로그인은 남아 있다. 일부 여정의 확인을 전체 pass로 올리지 않았다.

문서의 로컬 링크 69개에서 누락은 없었고 `git diff --check`를 통과했다. 정리 결과는 `final.json`에 기록했다. 앱 PID 50056과 재실행 54960, fixture 서버 50028과 53619가 모두 종료 코드 0으로 끝났고 해당 PID와 프로젝트 앱 프로세스가 남지 않았다. 종료 전 다운로드와 권한, 저장 대기, 생성 예약, 캡처, focus lease와 대기 요청은 0이었다. 새 검증 프로필의 Chromium 데이터 137,065,617바이트를 정리하고 북마크와 기록 JSON 1,013바이트만 남겼다. 실행 근거와 검증 파일은 보존했다. 기존 사용자 프로필과 다른 작업 자료는 변경하지 않았으며 커밋, 푸시, 배포는 하지 않았다.

## 2026-10-19, v16과 v17 최종 후보 실행

v16은 앱 `9b685cba41fa9d1f6c78415a0ee428bdd93cc9c9fe6f7981484be59604dc6b75`, CLI `da321cbfe2d47b48836c21fbaf9f89750c7e69cfc5efb23642c01b6f0a58a784`다. v17은 OOPIF 렌더러 종료 수정을 더한 앱 `909809680609d3fc847714f3be961bb0a3aa795d36af937ab7296ad1233e7d98`이고 CLI는 v16과 같다. 단위 테스트는 v17 소스에서 393개 통과했다. 모두 `NARU_MOCK_KEYCHAIN=1`과 별도 검증 프로필에서 실행한 로컬 CEF 통합 검사다.

| 여정 | v16 | v17 |
| --- | --- | --- |
| 01, 04, 06, 07, 12 | 통과 | 통과. `select_smoke` 첫 실행 1회가 `TASK_STALE_REVISION`으로 실패했고 이후 7회 연속 통과했다. 원인 미확인 |
| 05 CLI 부분 | 통과 | 통과 |
| 08 프레임 | 5번 항목 실패. OOPIF 렌더러 종료 뒤 `frames`가 45초 TIMEOUT | 통과. v14 충돌 재현 10종 각 15회, OOPIF 렌더러 종료 4가지 방식 각 10회에서 앱 종료와 TIMEOUT 0건 |
| 09 CLI 부분 | 통과. 권한 검사 2회 통과 | 통과 |
| 11 공개 읽기 | 통과 | 통과. 로그인 부분은 미실행 |
| 13 재시작 | 통과. SIGTERM 종료 뒤 쿠키 3회 유지, 마지막 탭 닫기 10회 멈춤 없음 | 통과 |
| 14 지속 사용 | 통과. 60분 31초, 10회 오류 0, footprint 195.6MiB에서 최고 221.2MiB, 유휴 뒤 213.0MiB | 미완료. 1회차 뒤 앱 메인 프로세스가 크래시 보고서와 로그 없이 사라졌다. 같은 시간대에 다른 검증 앱도 사라졌다. 사용자가 여러 창이 떠 있어 직접 종료했다고 확인해 앱 결함이 아니다. 단독 재실행은 사용자 요청으로 시작 직후 중지했다 |
| 02, 03, 05 UI, 09 UI, 10 | blocked. 이 세션의 상위 프로세스인 Antigravity IDE에 손쉬운 사용 권한이 반영되지 않아 `AXIsProcessTrusted`가 false다. 10번은 다크 모드 창 캡처만 실행했다 | 같은 이유로 미실행 |

- 렌더러 종료 수정: Chromium은 OOPIF 렌더러가 죽어도 child session을 떼지 않고 `Inspector.targetCrashed`만 보내며, 렌더러로 가는 명령은 응답 없이 보관한다. Naru는 이 이벤트를 버려 `Page.enable`과 `Page.getFrameTree` 응답을 기다렸다. child session 준비 첫 단계에 `Inspector.enable`을 보내 이미 죽은 session을 표시하고, 표시된 session은 건너뛴 뒤 결과를 `truncated:true`로 보고한다. 관련 파일은 `src/browser.rs`, `src/frame_sessions.rs`, `docs/devtools-routing.md`다.
- 다운로드 권한: 사용자가 Chromium 동작 유지를 선택했다. CLI click 한 번은 사용자 상호작용 한 번이며 한 클릭에서 나온 두 번째 다운로드는 권한을 기다린다.
- 정리: 사용자 요청으로 모든 실행을 중지했다. 검증 앱, fixture 서버, `caffeinate`가 모두 종료됐고 남은 Naru 프로세스는 없다. 검증 프로필 `.local-data/e2e-v16-*`, `e2e-v17-*`은 근거로 보존한다. 커밋은 하지 않았다.
- 근거: `artifacts/e2e-v16/`, `artifacts/e2e-v17/`. 추가한 검사는 `tests/frame_crash_smoke.py`, `tests/e2e_v16_*.py`, `tests/e2e_v17_*.py`다.
- 남은 일: 14번 v17 재실행, 손쉬운 사용 권한 반영 뒤 네이티브 UI 여정, 11번 Google 로그인 확인, 한글 IME 사용자 수동 확인, 커밋 여부 결정.

## 2026-10-19, OOPIF renderer 종료 뒤 frames 무응답 수정

v16(앱 `9b685cba…`)에서 OOPIF renderer를 SIGKILL로 끝내면 그 탭의 `frames`가 45초 뒤 `TIMEOUT`이었고 두 번째 호출도 같았다. Chromium은 죽은 child session을 detach하지 않고 `Inspector.targetCrashed`만 보낸다. renderer로 가는 명령은 frame이 다시 살아날 때까지 보관한다. Naru는 이 이벤트를 무시하고 그 session에 `Page.getFrameTree`나 `Page.enable`을 보냈다. 프레임 경로에는 요청 단위 45초 deadline 외의 감시가 없어서, CLI 제한과 같은 시점에 앱이 `TIMEOUT`을 답했다. 다시 연결한 session에는 crash 알림이 오지 않아 다음 호출도 같은 자리에서 멈췄다.

- 변경: child session 준비 첫 단계로 `Inspector.enable`(`FramePhase::CrashProbe`)을 보낸다. child의 `Inspector.targetCrashed`, `Inspector.detached`는 registry에 crashed를 표시하고 `Inspector.targetReloadedAfterCrash`는 해제한다. crashed session은 건너뛰고 `truncated:true`로 보고한다. 진행 중 inventory가 읽었거나 명령을 보낸 session이면 `FRAME_CHANGED`로 끝내고 추적을 정리하며, 그 session의 focus lease도 해제한다. root 이벤트는 무시해 `RENDERER_TERMINATED` 경로를 유지한다. 쓰지 않게 된 `Registry::ids`를 지웠다.
- 관련 파일: `src/browser.rs`, `src/frame_sessions.rs`, `docs/devtools-routing.md`
- 단위 테스트: `cargo test --locked` 393개 통과. 새 테스트는 `crashed_sessions_are_skipped_until_reloaded`, `child_renderer_crash_skips_a_probe_but_fails_an_observed_session`이다.
- 빌드: 격리 worktree release 빌드 성공. 앱 `157118ad1cc80c61361dd5a8ad9658b4111f64be13f5b348325320e174734b15`, CLI `1fca1e52aa4fdf2d6a583ab349375d0a497c2a6fe715b11f23c2b38e5b575826`.
- 로컬 CEF 통합 검사: 검증 프로필 `.local-data/e2e-wt-oopkill-data-1`에서 실행했다. 사전 frames 후 종료, frames 없이 종료, frames 진행 중 종료를 각 10회 반복했다. 30회 모두 종료 뒤 frames 두 번이 `ok`, root 1개, `truncated:true`였고 최대 12ms였다. 진행 중 요청은 10회 모두 `FRAME_CHANGED`였다. read, read-page, close, task finish도 모두 `ok`였고 앱은 계속 응답했다. `tests/frame_journey_smoke.py` 56개 검사와 `tests/frame_crash_smoke.py` 50개 결과가 모두 통과했다.
- 미확인: 다른 작업 탭이 같은 renderer를 공유했을 때의 동작은 재현하지 않았다. 처리 경로는 같지만 실측은 없다.
- 정리: 검증용 앱 프로세스와 fixture 서버를 종료했다. 다른 Naru 인스턴스는 건드리지 않았다.

## 2026-10-19, 메모리 측정의 footprint 복구와 e2e-v15 진행

e2e-v14의 14번 측정에서 footprint가 모든 시점에서 비었던 원인을 확인했다. Chromium 프로세스에 `vmmap -summary`를 실행하면 종료 코드는 0이고 `Physical footprint` 값도 나오지만, stderr에 "Can't examine target process's malloc zone PartitionAlloc_…"와 "Reason: failed to resolve canonical path …" 두 줄이 찍힌다. `scripts/measure-memory.py`는 stderr가 비어 있지 않으면 모두 `footprint_warning`으로 처리했다. malloc zone 분석 실패는 커널이 보고하는 physical footprint에 영향을 주지 않으므로 이 두 줄만 허용하고 다른 stderr는 계속 경고로 처리하도록 바꿨다.

- 관련 파일: `scripts/measure-memory.py`, `tests/memory_report_test.py`
- 단위 테스트: `python3 -m unittest tests.memory_report_test` 10개 통과. 허용 경고만 있을 때 값 반환, 다른 경고가 섞이면 `footprint_warning`을 확인한다.
- 로컬 실측: 검증 프로필 `.local-data/fp-probe`에서 v14 앱을 띄워 `--footprint`로 측정했다. 프로세스 6개 모두 값이 나왔고 합계 192.8MiB였다. 측정 뒤 앱을 종료하고 프로필을 지웠다.

### v14 바이너리 탐색 실행 (e2e-v15 러너)

모두 앱 `877bb599…`, CLI `a47eff17…`에서 실행한 로컬 CEF 통합 검사다. 이후 코드를 고쳤으므로 최종 판정 근거가 아니라 결함 탐색 결과다.

- 14번 지속 사용: `caffeinate`로 60분 24초 연속 실행했다. 10회 모두 기준으로 돌아왔고 충돌은 없었다. 자식 프로세스는 처음부터 끝까지 5개였다. footprint 합계는 191.5MiB에서 최고 213.1MiB, 15분 유휴 뒤 208.7MiB였고 열린 파일은 2회부터 190개로 고정됐다. e2e-v14의 6회 증가는 재현되지 않았다. 근거는 `artifacts/e2e-v15/j14/`.
- 08번 프레임: 9개 문서 유형에서 7개 입력과 hover, click의 DOM 결과, 부모 숨김과 비활성, 가림, stale·위조 ref 거절, 프레임과 본문·요소·옵션 상한을 확인했다. 결함은 OOPIF 추적 뒤 메인 탐색, 같은 프로세스로 전환, OOPIF 반복 생성, 깊이 9 이상에서의 앱 종료 4종과, 연결 수 초과 뒤 누락 목록을 truncated false로 보고한 1건이다. 근거는 `artifacts/e2e-v15/j08/`, 회귀 검사로 `tests/frame_journey_smoke.py`와 `tests/fixtures/frame_*.html`을 추가했다.
- 05번 CLI 부분: handoff 뒤 이전 revision은 `TASK_STALE_REVISION`, 진행 중이던 `wait`은 `TASK_CONTROL_LOST`, resume 뒤 옛 ref는 `STALE_REF`였고 B 작업은 계속 동작했다.
- 11번 공개 읽기: example.com, Wikipedia, MDN에서 읽기, 스크린샷, 링크 이동을 확인했다.
- 13번: 사용자 공간 방문만 기록되고 작업 페이지는 빠졌다. 재시작 뒤 기록과 localStorage가 유지됐고 이전 CLI 요청은 재실행되지 않았다. 북마크는 앱 종료 상태에서 파일을 직접 작성한 seeded 자료라 파일 보존만 확인했다. 결함은 SIGTERM 종료 뒤 쿠키 유실과 마지막 탭 닫기 뒤 종료되지 않는 경우 1회다.
- 09번 CLI 부분: 작업 탭의 연속 다운로드가 권한 대기 없이 진행됐다. 기존 검사가 2회 모두 실패했다.
- 근거는 `artifacts/e2e-v15/cli/`, 러너 스크립트는 `tests/e2e_v15_*.py`.

### 결함 수정 (v15 빌드)

- 프레임 추적 앱 종료: Chromium이 DevTools 관찰자를 순회하는 콜백 안에서 Naru가 응답 처리와 다음 명령 전송, `Target.setAutoAttach(false)`를 동기로 실행해 관찰자 목록을 바꿨다. 크래시 보고서 스택에 `on_dev_tools_message → devtools_result → … → send_session_command`가 남았다. `ControlObserver`의 두 콜백은 메시지를 복사해 UI 작업으로 넘기고, `control_message`가 탭과 브라우저를 다시 확인한 뒤 처리한다. 격리 빌드에서 재현 9종을 5회씩 실행해 앱 종료 0회, `observer_list` 로그 0줄이었고 연결 수 초과는 매번 `FRAME_LIMIT`이었다.
- SIGTERM 쿠키 유실: Chromium의 macOS SIGTERM 처리는 CEF 브라우저를 닫지 않고 끝낸다. `mac::quit_on_signals`가 SIGTERM, SIGINT, SIGHUP을 Quit 메뉴와 같은 `close_all`로 보내고 10초 안에 끝나지 않으면 강제 종료한다. 격리 빌드에서 SIGTERM 3회, SIGINT와 SIGHUP 각 1회 쿠키가 유지됐고 `control.sock`이 지워졌다.
- 마지막 탭 닫기 뒤 종료 실패: 비정상 종료 이력이 있으면 AppKit이 시작 시 창 복원 질문을 modal로 띄우고, 그 안에서 Chromium의 종료 요청이 modal loop만 끝냈다. `ApplePersistenceIgnoreState`를 등록 기본값으로 넣어 이 질문을 막고, 탭이 없을 때 `close_all`이 종료를 요청한다. 수정 후 크래시 뒤 재실행 10회와 마지막 탭 닫기 20회에서 멈춤은 없었다.
- 연속 다운로드 권한: CLI click이 실제 Chromium 포인터 입력이라 클릭마다 사용자 상호작용으로 집계되는 정상 동작이었다. 사용자가 이 동작을 유지하기로 정했다. `tests/download_permissions_smoke.py`를 클릭 한 번에 두 요청을 보내는 방식으로 바꾸고 `docs/pointer-input.md`에 기록했다. v15 빌드에서 2회 통과했다.
- 두 수정을 합친 v15 빌드는 앱 `4948423fe107c9e8b72ce9fad2fc62bd0c83da7ac19d053540af9557db2adb94`, CLI `edf9a74224c0bc94a7f6434a49aa2841818db73d4695c942b9cf43b45b2e6e26`이다. Fable 리뷰에서 blocker는 없었고, 두 번째 종료 신호의 즉시 종료와 렌더러 종료 콜백의 DevTools 전송 지연을 반영 결과는 아래 v16 절에 있다.

### 리뷰 반영 (v16 빌드)

- 두 번째 종료 신호: Chromium 처리기는 두 번째 SIGTERM, SIGINT에 바로 끝났는데 v15는 10초 감시만 기다렸다. `quit_on_signals`의 `received`가 `AtomicBool`로 두 번째 신호를 구분해 `_exit(128+신호)`로 끝낸다. 첫 신호는 기존처럼 `close_all`과 10초 감시를 쓴다.
- 렌더러 종료 콜백: `on_render_process_terminated`가 콜백 안에서 `renderer_terminated`를 실행해 종료 중인 agent에 `Emulation.setFocusEmulationEnabled(false)`, `Runtime.disable`, `Target.setAutoAttach(false)`를 보낼 수 있었다. `RendererTerminatedTask`를 UI 작업으로 넘기고, 작업에서 탭이 남아 있고 같은 브라우저인지 다시 확인한 뒤 기존 처리를 실행한다. 콜백 안에서 미리 해야 하는 상태 변경은 없다고 판단했다. 그 사이 실행된 요청은 같은 탭의 pending으로 남으므로 작업이 `RENDERER_TERMINATED`로 답한다. 이 부분은 코드 경로로 확인했고 실측하지 않았다. 콜백은 반환값이 없고 다른 처리도 없다.
- 주석: `do_close`의 관찰 해제 이유를 UI 작업 처리 기준으로 고쳤다. 동기 ACK를 전제로 한 주석 두 곳(`stop_frame_tracking`, `send_focus_message`)을 현재 동작 기준으로 바꿨다. 코드는 그대로다.
- 단위 테스트 `posted_control_message_revalidates_tab_and_ignores_malformed_bytes`: 대기 요청의 응답 전달, 탭 제거 뒤 같은 메시지의 무응답, 잘린 JSON 무시를 확인한다.
- 관련 파일: `src/mac.rs`, `src/browser.rs`

| 구분 | 내용 | 결과 |
|---|---|---|
| 단위 테스트 | `cargo test --locked` (`.cache/bin` PATH, `CEF_PATH=.cache/cef`) | 391개 통과, 실패 0 |
| 릴리스 번들 | `scripts/dev.sh --release --build-only` | 성공, 기존 dead_code 경고 1건. 앱 `9b685cba41fa9d1f6c78415a0ee428bdd93cc9c9fe6f7981484be59604dc6b75`, CLI `da321cbfe2d47b48836c21fbaf9f89750c7e69cfc5efb23642c01b6f0a58a784` |
| 로컬 CEF 통합 | SIGTERM 1회, 유휴 프로필 | 0.13초에 정상 종료(rc 0), `control.sock` 제거 |
| 로컬 CEF 통합 | 사용자 탭 `single-window.html`에서 CLI click으로 beforeunload를 켠 뒤 SIGTERM 2회 | 첫 신호 뒤 3초간 실행 유지, 두 번째 신호 뒤 0.035초에 종료(rc 143). `_exit`라 `control.sock`이 남아 수동 삭제 |
| 로컬 CEF 통합 | 작업 탭 `check.html`에서 `frames` 뒤 탭 렌더러 `kill -9`, 3회 | 매회 앱 생존, `read`는 `RENDERER_TERMINATED`, `reload`와 `wait load` 뒤 `read` 성공. 앱 로그 `observer_list` 0줄 |

- 작업 탭에서 beforeunload를 켠 첫 시도는 첫 SIGTERM 뒤 2.81초에 정상 종료됐다. 숨은 작업 탭에서 확인 창이 뜨지 않은 것으로 보이며 원인은 확인하지 않았다.
- 탭 렌더러 식별: 탭을 열면 Chromium이 대기 렌더러를 쓰고 새 대기 렌더러를 띄워 "새 자식 프로세스"가 탭 렌더러가 아니었다. 후보를 하나씩 종료하며 `tabs`의 렌더러 상태가 `terminated`가 될 때까지 확인했다. 2회차에는 탭과 무관한 렌더러 2개를 먼저 종료했고 앱은 살아 있었다.
- 검증 프로필 `.local-data/v16-check`, 스크립트와 로그는 세션 scratchpad에 있다. fixture 서버와 앱은 모두 종료했다. 커밋은 하지 않았다.

### 크래시 보고서 정리

2026-10-18부터 19일까지 `~/Library/Logs/DiagnosticReports`의 Light Browser 보고서는 26개다. 10월 19일 18시대 10개는 08번 러너가 v14에서 만난 프레임 추적 종료이고, 18시 47분 7개는 수정 전 빌드로 일부러 재현한 것이며, 18시 59분과 19시 1분 2개는 멈춤 재현을 위해 SIGSEGV를 보낸 것이다. 10월 18일 20시대 6개는 탭 닫기 충돌 재현 시간과 겹친다. 10월 18일 14시 39분 1개는 원인을 확인하지 못했다.

- 정리: e2e-v15 러너와 격리 빌드 앱, fixture 서버는 모두 종료됐다. worktree `.local-data/wt-fix`, `.local-data/wt-frames`는 소스 변경을 본 트리에 합친 뒤 제거했다. 검증 프로필 `.local-data/e2e-v15-*`, `wt-*-data-*`는 근거로 보존한다.

## 2026-10-18, E2E e2e-v14 로컬 CEF 통합 실행

요청 범위는 [E2E 체크리스트](ego-parity-e2e.md)의 14개 여정을 같은 앱 바이너리에서 확인하는 것이다. 앱 SHA-256은 `877bb5993ae3e7f9383bc8c690a6fabb066fa3642140c34c692c2f74b02c24eb`, CLI SHA-256은 `a47eff17b9f5f92358624974848b0e6078df6a6be66801230ab5285e9aa0eb5f`이다. 실행 ID는 e2e-v14이고 검증 프로필은 `.local-data/e2e-v14`다. `NARU_MOCK_KEYCHAIN=1`로 실행했다. 실제 사용자 프로필은 건드리지 않았고 프로필 폴더의 수정 시각도 바뀌지 않았다. 증거 파일은 `artifacts/e2e-v14/`에 있다. 앱 충돌은 0건이었다.

| 여정 | 검증 구분 | 결과 |
| --- | --- | --- |
| 01 시작과 사람 공간 | 로컬 CEF 통합 | 통과. task open 뒤 사용자 공간 `user`와 활성 탭 1이 유지됐다. 메인 프로세스 pid 85395 하나 |
| 04 배경 탐색 | 로컬 CEF 통합 | 통과. `spaces_smoke` 10개 검사, `single_window_smoke` 6개, `tasks_smoke` 통과. `wait` 시간 초과는 `WAIT_TIMEOUT`, elapsedMs 1033으로 응답했고 `navigationContinues`가 true였다 |
| 06 클릭과 값 변경 | 로컬 CEF 통합, 자체 fixture | 통과. 숨은 탭에서 로드 뒤 나타난 대상을 포함한다. hover만 실행하면 click과 누름, 해제가 0회였다. `native_fill`, contenteditable, `insert_text`, check, select 통과 |
| 07 키 이동 | 로컬 CEF 통합 | 통과. `native_press_smoke` 42개 키 형식 |
| 08 iframe과 Shadow DOM | 로컬 CEF 통합, 자체 fixture | 일부 확인, pending. `shadow_smoke`와 다른 출처 iframe 3회 반복에서 open, `frames`, `read-page` 문서 2개, `read-frame`, fill 뒤 재읽기 검증, frame ref click 응답 확인, 탭 닫기 뒤 앱 유지를 확인했다. frame ref click의 페이지 효과는 확인하지 않았다 |
| 11 공개 사이트와 로그인 | 외부 사이트 읽기 | 공개 읽기 통과. Wikipedia 링크 클릭으로 URL이 바뀌었고 뒤로 가기로 복원됐다. 네이버 메인은 읽기와 클릭을 확인했으며 CAPTCHA는 나오지 않았다. 로그인 서비스 부분은 blocked |
| 12 실패 복구 | 로컬 CEF 통합, 자체 탭과 서버 | 통과. `navigation_recovery_smoke`와 `controls_smoke`에서 사용자 선택이 유지됐다. 검증 탭의 renderer 종료 뒤 read는 `RENDERER_TERMINATED`를 반환했고 사용자 탭은 영향을 받지 않았다. reload로 복구됐다 |
| 13 종료와 재시작 | 로컬 CEF 통합 | 일부 확인, pending. `restart_smoke` 2회. 같은 앱 해시에서 쿠키와 localStorage가 유지됐고 정상 종료했다 |
| 02, 03, 05, 09, 10 | 네이티브 UI | pending. AppKit UI 절반에 필요한 macOS 손쉬운 사용 자동화를 사용자가 보류했다 |
| 14 지속 사용 | 로컬 CEF 통합 | 부분 확인, pending. 10회 모두 기준으로 돌아왔고 충돌은 없었다. 약 45분 만에 멈춰 60분을 채우지 못했고 footprint는 비었다. 6회에서 자식 프로세스가 6개에서 9개(utility 2개, renderer 1개)로 늘었다 |

이 실행은 네이티브 AppKit UI 확인이 아니다. 외부 사이트 전체 호환성의 근거도 아니다. 실제 서명 식별자는 여전히 없으므로 릴리스 서명 확인으로 보지 않는다. 검증 모드의 키체인 우회로 앱 시작이 막히던 문제는 해소됐다.

작업은 세 개의 git worktree(`.local-data/wt-crash`, `.local-data/wt-input`, `.local-data/wt-readpage`)에 나누어 진행했고 변경마다 리뷰를 거쳤다. 정리 상태는 다음과 같다. 여정 14가 끝날 때까지 앱을 `.local-data/e2e-v14` 프로필에서 실행 유지하며 사용자 탭 1개가 열려 있다. 14 종료 뒤 러너가 fixture 서버를 끄고 자체 탭 35를 닫았으며 resources 값이 모두 0임을 확인했다. 앱 pid 85395는 2026-10-19 확인 때 이미 종료돼 있었다. 사용자 탭 1은 실행 중 Google 검색 결과(검색어 "구매대행 반자동", 창 폭 1840)로 바뀌어 있었다. 러너는 그 탭에 명령을 보내지 않았으므로 사람이 직접 검색한 것으로 보이며, 6회의 자식 프로세스 증가와 시점이 겹치는지는 확인하지 못했다. 빌드 중간 산출물은 `scripts/dev.sh`가 정리했다. worktree는 커밋 결정 뒤 제거한다. 커밋, 푸시, 배포는 하지 않았다.

## 2026-10-18, 입력 포커스 임시 준비와 숨은 탭 포인터 입력 연결

사용자가 숨은 탭 입력에는 임시 focus emulation을 쓰기로 했다. click과 hover가 이 lease를 사용한다. lease는 move 확인부터 press와 release까지 이어진다. 루트 키나 포인터 press가 전달된 뒤 탐색이 일어나도 성공 결과를 유지한다. 이 과정에서 `restart_smoke`의 press PAGE_CHANGED 경합도 함께 고쳤다. `docs/focus-input.md`와 `docs/pointer-input.md`를 갱신했고 `tests/native_click_smoke.py`와 `tests/fixtures/native_click.html`에 숨은 뒤 로드되는 탭 사례를 추가했다.

| 구분 | 내용 | 결과 |
| --- | --- | --- |
| 단위 테스트 | `cargo test --locked --lib` | 390개 통과. 이전에 실패하던 `fill_*` 테스트 6개는 오래된 테스트 helper를 갱신해 통과 |
| 모의 DOM | `node --test tests/dom_bridge.test.mjs` | 101개 통과, 27개 실패. 수정 전부터 있던 실패이며 이번 변경으로 늘지 않았다 |
| 로컬 CEF 통합 | e2e-v14의 `native_click_smoke` 숨은 탭 사례 | 통과 |

## 2026-10-18, 입력 실패 응답과 fill 대상 바인딩, insertion 범위 보정

`FillPreflight` 실패는 `details`의 attempted를 false로, `outcomeUnknown`을 false로 응답한다. dispatch는 focus lease를 시작하기 전에 fill 대상을 먼저 바인딩한다. 대상 바인딩이 실패하면 PAGE_CHANGED로 응답한다. `src/dom_bridge.js`의 insertionSnapshot은 Range prototype 체인을 따라 메서드를 찾도록 바꿨다. Chromium 156에서 NodeRange와 AbstractRange에 메서드가 나뉘어 있기 때문이다. 관련 파일은 `src/browser.rs`와 `src/dom_bridge.js`다.

## 2026-10-18, 탭 닫기 크래시 수정

`frames`로 OOPIF 하위 session을 붙인 탭을 닫으면 `EXC_BAD_ACCESS`로 앱이 종료됐다. 로그에는 ObserverList 재진입이 남았다. 탭을 정리하는 동안 하위 session의 detach 콜백이 DevTools 명령을 다시 보냈기 때문이다. `LifeHandler::do_close`에서 teardown 전에 탭의 DevTools 관찰자 Registration을 먼저 해제하도록 바꿨다. `on_before_close`는 여전히 대기 중인 요청에 `TAB_CLOSED`로 응답한다.

| 구분 | 내용 | 결과 |
| --- | --- | --- |
| 로컬 CEF 통합 | 격리 worktree 빌드에서 OOPIF 탭 닫기 | 5회 통과 |
| 로컬 CEF 통합 | e2e-v14의 frames 뒤 탭 닫기 | 3회 통과 |

남은 한계는 `frames`가 호출마다 재진입 확인 로그를 한 줄 남기는 것이다. 충돌은 없었다. 후속 과제로 둔다.

## 2026-10-18, read-page 허용 목록과 서버 거절 ID 표시 수정

task 안에서 `read-page`를 실행하면 CLI가 `RESPONSE_ID_MISMATCH`를 표시했다. 프로토콜 허용 목록에 `read_page`가 없어서 서버가 요청 ID를 `invalid`로 돌려보냈고, CLI는 이를 응답 ID 불일치로 해석했다. `src/protocol.rs`의 허용 목록에 `read_page`를 추가했다. `src/browser.rs`에서는 ID가 `invalid`인 서버 거절을 서버의 오류 코드 그대로 표시하도록 바꿨다. 이때 `INVALID_REQUEST_ID` 상수와 `ProtocolError.code` 문자열을 쓴다. 이 경우 `outcomeUnknown`은 false다.

| 구분 | 내용 | 결과 |
| --- | --- | --- |
| 로컬 CEF 통합 | e2e-v14의 task `read-page`, 08번 여정 | 통과 |
| 리뷰 | 변경마다 수행 | 완료 |

## 2026-10-18, 누적 구현의 커밋과 푸시 준비

현재 변경을 우선 커밋하고 푸시해 달라는 요청에 따라 Cargo.toml, README, src, docs, tests의 변경 파일 46개를 커밋 범위로 확인했다. Chromium 키보드 입력과 hover, insert, 프레임 읽기와 조작, 여러 문서의 공통 읽기 예산, 입력 포커스 준비와 해제, UI 상태 관찰 보완 및 관련 문서와 검사 자료를 포함한다. 이번 작업에서는 제품 동작을 추가로 수정하지 않았다. 목적지는 기존 `origin/codex/navigation-readiness`이며 GitHub 조회로 `FEABE/naru-browser`가 비공개임을 확인했다. 기본 브랜치 병합이나 배포는 요청 범위에 포함하지 않는다.

현재 제품 소스와 Cargo 파일 36개의 SHA-256을 v10 통합 후보 기록과 다시 대조해 모두 일치함을 확인했다. `git diff --check`도 통과했다. 직전 release 빌드와 정적 검사의 성공 기록을 유지하며 새 빌드, 단위 검사, 모의 DOM 검사, 로컬 CEF 통합 검사, 네이티브 UI 확인, 외부 사이트 및 E2E 검사는 이번에 실행하지 않았다. 사용자 요청대로 반복 회귀 실행을 늘리지 않았다. E2E 01의 안정 실행 서명과 11의 대표 로그인 서비스 확인은 blocked, 다른 12개 여정은 pending이다. 이 상태를 브라우저 완성이나 실제 입력 검증 완료로 해석하지 않는다.

빌드 생성물, 로컬 evidence artifacts, 캐시, 사용자 프로필은 기존 ignore 규칙에 따라 커밋 대상에서 제외한다. 문서에 기록한 로컬 artifacts는 원격 저장소에 포함되지 않는다. 앱이나 서버를 실행하지 않았고 새 빌드 생성물도 만들지 않아 추가 프로세스 종료나 생성물 정리는 없었다. 사용자 데이터는 보존했다. 이 기록은 커밋 직전에 작성했으며 실제 커밋 식별자와 원격 반영 여부는 후속 Git 실행 결과로 확인한다.

## 2026-10-18, 입력 포커스 준비·해제와 대상 관찰, 통합 후보 v10

직전 goal turn은 공통 예산 read-page와 v9 빌드를 완료한 progress다. 이번에는 첫 Chromium 입력의 페이지 포커스 전환을 준비 앞으로 옮기는 보완과 일반 fill의 입력 대상 관찰을 구현했다. Aside의 일상 사용성과 ego lite의 사용자·AI 공간 및 CLI를 갖춘 독립 Naru 완성 goal은 active다. 전체 회귀 묶음을 반복하지 않고 제품 구현, 한 번의 통합 release 빌드, 같은 바이너리의 E2E 목록 갱신을 진행했다. 독립판 다음 같은 Rust·CEF 구현을 Preseller에 내장하는 순서를 유지한다.

`src/browser.rs`에 정확한 Browser·session·token의 임시 focus lease를 연결했다. press와 insert, Chromium fill은 enable ACK 뒤 원 ref를 새로 준비하고 기존 입력 한 번, disable ACK, 읽기 전용 사후 확인 순서로 처리한다. 프레임 경로는 기존 Runtime 정리도 이어간다. fill은 먼저 inspectOnly로 종류와 경로를 읽고 expectedInput·expectedKind를 고정한다. legacy DOM fill에는 임시 설정을 적용하지 않아 기존 값 확인 뒤 추가 blur를 만드는 경로를 피했다. CEF set_focus, 사용자 공간·선택 변경, 새 UI renderer나 저장소는 추가하지 않았다.

탭당 하나·전체 최대 16개의 lease는 일반 pending이 없어져도 정리 책임을 유지한다. enable sender 재진입 중 취소되면 반환까지 disable 전송을 보류한다. enable ACK 이전의 disable만으로 정리 완료를 판단하지 않으며 확정된 enable 다음 disable을 최대 한 번 더 보낸다. 설정 정리는 최대 두 번이고 입력 재전송은 없다. ACK 한도는 5초이며 활성 lease가 있을 때 전역 감시 task 하나가 1초마다 기존 45초 수명을 확인한다. 정리 실패에는 요청 텍스트·continuation·프레임 command와 snapshot을 해제하고 필요한 session·root object group만 보존한다. 새 페이지 조작은 막고 닫기·중지·대기는 기존 경로를 유지한다.

소스 검토에서 탭 닫기 중 Reply의 STATE 재진입, host getter 이후 취소된 준비의 재전송, 정확한 child session 종료 후 탭 전체가 실패 격리 상태로 남는 경계를 찾아 수정했다. 탭 종료는 borrow 밖에서 최종 오류와 자원을 정리한다. 준비를 예약하기 직전 임시 상태를 다시 확인한다. Registry가 확인한 제거 subtree에 원 child session이 포함되면 session-destroyed 오류로 끝내고 기존 frame 정리를 이어간다. root나 무관한 child를 이 알림으로 해제하지 않으며, 엔진의 비동기 session 폐기를 disable ACK나 renderer reset 완료로 표시하지 않는다. 최종 읽기 검토에서 추가 확정 차단 결함을 찾지 못했지만 런타임 통과를 뜻하지 않는다.

`src/dom_bridge.js`의 일반 input·textarea fill은 insert와 임시 receipt 하나를 공유한다. beforeinput·input 각각의 첫 trusted 이벤트를 passive capture하고 원 ref·종류·요청값·세대를 확인한다. 다른 대상 관찰은 원 대상의 최종 값 일치보다 우선 INPUT_TARGET_MISMATCH로 반환한다. 성공은 기존 값 검증과 inputTarget matched/unconfirmed를 구분하며 token·입력 원문을 새 CLI 필드로 노출하지 않는다. 새 읽기·동작·소비·5초 단발 만료에서 최대 네 listener를 해제한다. 사람 입력 식별, 다른 문서 이벤트의 완전 관찰이나 이벤트 차단 기능은 아니다. contenteditable fill의 기존 값 확인과 insert·press의 계약을 유지한다.

고정 Chromium·CEF·V8 원문 18개의 포커스·visibility·session 폐기 호출 근거를 `artifacts/focus-emulation-v1-sources.json`에 기록하고 `docs/focus-input.md`를 추가했다. README, architecture, product-plan, preseller-integration, native-fill-control, keyboard-input, insert-text, frame-input, devtools-routing, memory와 ego-parity-e2e를 갱신했다. 06·07 여정에 첫 포커스 전환과 지연 스크립트, disable 부작용, 취소와 늦은 ACK, 입력 중 탭 닫기, 정확한 OOPIF session 종료 및 자원 회수를 추가했다. 문서 패치 한 번은 존재하지 않는 제목 때문에 적용 전에 거절됐고 실제 제목 확인 후 재적용했다. 제품 소스나 실행 실패와 구분한다.

`CARGO_NET_OFFLINE=true CARGO_BUILD_JOBS=2 ./scripts/dev.sh --release --build-only`를 한 번 실행했다. 세션 62956은 91.15초, exit 0이며 소스·Cargo 파일 36개의 빌드 전후 해시가 같다. 기존 Registry의 테스트 전용 remove·len·is_empty에 dead_code 경고 한 묶음만 있었고 빌드 오류는 없다. `artifacts/ego-parity-v10-build.json`과 로그가 근거다. 앱 SHA-256은 `feae3608a01f3b68e3b030b23b525808a7c59b47484f5155da362ae6b653913a`, 번들 CLI는 `a18e943813b2fbd72efbe020fef4c1e83c381d148a8b29c8256283d972f0875a`다. CLI help의 정상 JSON도 확인했으며 브라우저 입력 성공으로 계산하지 않는다.

빌드 스크립트가 이전 번들을 교체하고 Cargo 중간 파일 1,271개·표시 크기 684.0MiB를 정리했다. `artifacts/ego-parity-v10-postbuild.json`의 읽기 전용 조회에서 프로젝트 dist 앱 PID, 임시 앱 번들과 임시 실행 파일 보관 폴더는 0개다. target에는 최신 실행 파일 세 개만 남았다. 서명은 local.lightbrowser.dev ad hoc이며 실행용 identity는 미지정이다. 기존 실행 보호를 유지해 앱·서버·검증 탭을 시작하지 않았고 사용자 프로필·자료를 보존했다. 키체인 재승인 질문이나 보안 설정 우회는 하지 않았다.

cargo fmt, node --check와 git diff --check를 확인했다. 최종 소스·문서 해시와 링크 확인은 `artifacts/ego-parity-v10-final.json`에 기록한다. 단위 검사·모의 DOM·로컬 CEF 회귀·네이티브 UI·외부 사이트·E2E는 이번에 실행하지 않았다. E2E 01은 안정 실행 서명, 11은 대표 로그인 서비스가 없어 blocked이며 나머지 12개 여정은 pending이다. 첫 포커스 전환의 실제 개선 효과, 지연 focus·layout 경합, 사용자 화면 보존, IME·로그인·지속 메모리와 지원 밖 프레임 기하는 남아 있다. 탭 복원·확대 보류를 유지하고 독립판·Preseller 내장 완료를 선언하지 않는다. 커밋·푸시·배포는 하지 않았다.

## 2026-10-18, 공통 예산의 여러 프레임 읽기와 통합 후보 v9

직전 goal turn은 프레임 포인터와 v8 빌드를 완료한 progress다. 이번에는 AI가 탭의 여러 문서를 한 요청으로 읽고 각 문서 참조로 후속 조작을 이어가도록 구현했다. Aside의 일상 사용성과 ego lite의 사람·AI 공간 및 CLI를 갖춘 독립 Naru 완성 goal은 active다. 독립판 다음 같은 Rust·CEF 구현을 Preseller에 내장하는 순서를 유지하며, 전체 회귀 반복 대신 구현·통합 빌드·같은 앱의 E2E 목록을 진행했다.

`src/protocol.rs`와 `src/cli.rs`에 `read-page TAB`, task run 및 wire의 read_page를 추가했다. 기존 read·read-frame 형식은 유지한다. `src/browser.rs`는 새 frame inventory의 단일 root와 부모 연결을 확인하고 루트부터 너비 우선으로 최대 16개 문서를 시도한다. 각 문서를 정확한 session·unique context에서 재귀 없이 읽어 자식 본문을 중복 합산하지 않는다. documents와 unread, 전체 frames 메타데이터, 합산 budget·truncated·atomic:false를 반환한다. 시작 전 body·readyState 검사에서만 PAGE_LOADING을 사용량 0인 개별 누락으로 처리하고 Runtime disable ACK 뒤 다음 문서를 읽는다. 나머지 오류나 frame/context/소유권 변경은 전체 실패로 처리한다.

`src/dom_bridge.js`의 readFrameActions에 여섯 정수 한도를 받고 budgetLimit·budgetUsed를 반환하는 경로를 추가했다. 전체 본문 12,000 UTF-16 단위·요소 120개·walk 방문 10,000회·열린 root 64개·옵션 240개·로컬 iframe metadata 16개를 공유한다. 0 예산은 해당 수집을 제한하며 label의 반복 순회도 nodes에 포함한다. 문서 깊이 64와 조상·스타일·포커스 검사의 기존 제한은 별도다. 읽기 문자열이 emoji의 surrogate pair 중간에서 잘리는 경계를 소스로 확인해 본문 중간 샘플·최종 붙임과 compact·URL·제목에 작은 prefix 함수를 적용했다. 액션의 원문 입력이나 선택 영역 처리에는 적용하지 않았다.

새 `src/page_read.rs`를 `src/lib.rs`에 등록했다. Rust는 받은 limit가 실제 remaining과 같은지, usage가 범위 안인지, 본문 UTF-16 길이와 요소·옵션·metadata 배열 수가 맞는지 확인한 뒤 차감한다. nodes·roots는 보고된 값의 범위 검사이며 전체 DOM 작업량 실측으로 해석하지 않는다. 각 문서 응답의 직렬화 바이트 합계와 최종 Response envelope를 각각 2MiB로 제한하고 크기 계산용 전체 문자열 사본을 만들지 않는다. 새 의존성·저장소·UI renderer·상주 프로세스·반복 타이머는 추가하지 않았다.

native 참조는 최신 묶음 하나, 최대 16개 문서 binding·전체 120개 ref로 바꿨다. 요청 token과 문서 ordinal을 ref 접두어에 넣고, 액션은 ref가 속한 원 snapshot을 찾아 같은 frame/session/context·문서 세대를 재확인한다. 이미 수집한 문서의 ContextTracker도 후속 Runtime 이벤트를 받는다. 전체 active token을 유지하고 마지막 disable ACK·현재성·응답 크기 검사 뒤에만 참조를 한꺼번에 게시한다. 시작 전 TAB_BUSY·PAGE_LOADING 거절은 기존 묶음을 보존하지만, 새 읽기가 inventory 준비 단계에 접수되면 이전 묶음을 무효화하며 후속 실패에서 복구하지 않는다. 본문은 native snapshot에 보관하지 않는다. 각 world의 약한 DOM Map과 bridge는 즉시 모두 삭제하지 않으며 실제 누적 RAM은 미측정이다.

분담한 읽기 검토에서 추가 확정 차단 결함은 찾지 못했다. cargo fmt --all --check, node --check src/dom_bridge.js, git diff --check와 문서 로컬 링크를 확인했다. 단위·모의 DOM·로컬 CEF 회귀·네이티브 UI·외부 사이트·E2E는 실행하지 않았다. 소스 검토와 빌드가 실제 CEF callback 순서, 입력 효과와 사용자 화면 보존을 입증하지 않는다.

`CARGO_NET_OFFLINE=true CARGO_BUILD_JOBS=2 ./scripts/dev.sh --release --build-only`를 한 번 실행했다. 세션 88895는 95.05초, exit 0이며 제품 소스와 Cargo 파일 36개의 전후 해시가 같다. 기존 registry의 테스트 전용 remove·len·is_empty에 dead_code 경고 한 묶음이 있었고 빌드 오류는 없다. `artifacts/ego-parity-v9-build.json`과 로그가 근거다. 앱 SHA-256은 `00dcd205ebc95d23b189084d8e0241f17101efd0f88cfc0a792bdc4d0f0e1474`, 번들 CLI는 `2fcc1bd8b8157db9be2bd102eb6556ae985ca1c45e5462281f4cd6d52acf4c1a`다. 번들 CLI help의 정상 JSON과 read-page 표시를 확인했으며 실제 페이지 실행과 구분한다.

빌드 스크립트가 이전 번들을 교체하고 Cargo 중간 파일 1,271개·표시 크기 683.5MiB를 정리했다. 읽기 전용 후속 조회에서 프로젝트 dist 앱 PID·임시 앱 번들·임시 실행 파일 보관 폴더는 0개이며 target에는 최신 실행 파일 3개만 남았다. `artifacts/ego-parity-v9-postbuild.json`에 해시·서명·도움말과 정리를 기록했다. 서명은 local.lightbrowser.dev ad hoc이고 실행용 identity는 미지정이다. 기존 실행 보호를 유지해 앱·서버·검증 작업·탭을 시작하지 않았고 사용자 프로필과 저장 자료를 보존했다. 키체인 재승인을 반복 질문하거나 보안 설정을 우회하지 않았다.

`docs/page-read.md`를 추가하고 README·architecture·product-plan·preseller-integration·frame-read·frame-input·devtools-routing·iframe-control·memory·ego-parity-e2e를 갱신했다. E2E 08에 중복 없는 여러 문서 읽기, 여섯 공유 예산과 emoji 경계, 17번째 문서, 묶음 내 서로 다른 문서 참조, 부분 로딩과 전체 실패·정리·배경 선택 보존을 추가했다. 최신 후보·앱과 CLI 해시를 v9로 고정했으며 01·11은 blocked, 나머지는 pending이다.

지원 밖 프레임 좌표 변환과 지연 focus 경합 예방, 최신 실제 앱의 CEF·네이티브 UI·사용자 화면 보존·로그인·IME·지속 메모리 E2E는 남아 있다. 안정 실행 서명도 해결된 것으로 표시하지 않는다. 탭 복원·확대 보류를 유지하고 독립 브라우저나 Preseller 내장 완료를 선언하지 않았다. 커밋·푸시·배포는 하지 않았다.

## 2026-10-18, 프레임 포인터 좌표·가림 검사와 통합 후보 v8

직전 goal turn은 원문서에 묶인 입력 5개와 v7 빌드를 완료한 progress다. 이번에는 남은 교차 출처 frame ref의 click·hover를 연결했다. goal은 active이며 Aside의 일상 사용성과 ego lite의 사람·AI 공간 및 CLI 협업을 갖춘 독립 Naru 완성 목표를 유지한다. 전체 회귀 묶음 대신 제품 구현·한 번의 통합 빌드·같은 앱의 E2E 목록 갱신을 진행했다. 독립판이나 Preseller 내장을 완료로 표시하지 않는다.

`src/dom_bridge.js`에 frameAct의 로컬 geometry·후보 단계와 inspectFramePointer를 추가했다. leaf 요소와 최대 32개 부모의 content box·viewport를 교차해 공통 clip을 구하고, 최대 9개 로컬 후보를 부모별 좌표 변환과 실제 hit-test로 거른다. 부모 문서의 정확한 owner를 사용하며 contentDocument에 접근하지 않는다. 가시 후보를 못 찾으면 leaf scrollIntoView 한 번 후 전체 geometry를 다시 수집하고, 두 측정의 후보 생성 합계는 최대 18개다. 후보 목록은 로컬·현재 부모 좌표 각 최대 9개이며 부모별 hit-test와 사후 단일점 검사의 횟수와 구분한다. 새 노드 강한 참조·renderer·프로세스·저장소·반복 타이머는 추가하지 않았다.

`src/browser.rs`는 원 leaf 좌표와 root 전송 좌표를 분리한다. 마우스 이동과 클릭 전송·ACK는 root session을 사용하고 DOM 검사는 원 child session·unique context·ref binding을 유지한다. mouseMoved 뒤 원 leaf 위치, 모든 부모의 hit-test와 같은 root 위치, 원 leaf 위치를 다시 확인한 다음 hover를 끝내거나 click press/release를 보낸다. 이동 뒤 후보 재선택·스크롤·입력 재시도는 없다. 부모 backendNodeId와 context를 유지하고 각 pass의 원격 객체 group을 해제한다. target으로 돌아올 때 Runtime replay로 원 context를 재확인하며 같은 session의 모든 관련 tracker에 생성·파괴·초기화 이벤트를 전달한다. 이미 이동을 보낸 뒤 부모 검사에서 실패해도 attempted·outcomeUnknown을 보존하며 성공은 기존 Runtime disable ACK 후 한 번만 보낸다.

새 `src/frame_pointer.rs`는 유한 좌표·양수 스케일·사각형 교집합과 역변환, 후보 index 부분집합·중복·좌표 계산을 제한된 값으로 검사하며 `src/lib.rs`에 등록했다. DOM hit-test를 대신하지 않는다. `src/cli.rs` 도움말과 read-frame 응답은 지원 action 7개와 pointerActionsSupported:true로 갱신했다. 회전·skew·perspective·SVG 조상·motion path·pinch 및 visualViewport offset은 기존 지원 기하 밖이며, 순차 검사와 비동기 스크롤 사이의 레이아웃 변화·지연 focus 경합은 남아 있다. 엔진 ACK만으로 실제 클릭 대상과 페이지 효과를 확정하지 않는다.

고정 Chromium 원문 21개에서 root/child 좌표와 widget 라우팅, local-root quads의 한계, scale factor와 remote 조상 스크롤을 확인해 `artifacts/frame-pointer-v1-sources.json`에 URL·해시·줄별 근거를 기록했다. source-only 검토에서 추가 확정 차단 결함은 찾지 못했다. cargo fmt, node --check src/dom_bridge.js, git diff --check와 문서 링크를 확인했다. 단위 검사·모의 DOM·로컬 CEF 회귀·네이티브 UI·외부 사이트·E2E는 실행하지 않았다. 원문 조사·빌드와 실제 Naru 실행 성공을 구분한다.

`CARGO_NET_OFFLINE=true CARGO_BUILD_JOBS=2 ./scripts/dev.sh --release --build-only`를 한 번 실행했다. 세션 59083은 91.49초, exit 0이며 제품 소스와 Cargo 파일 35개의 빌드 전후 해시가 일치한다. 기존 registry의 테스트 전용 remove·len·is_empty에 dead_code 경고 한 묶음이 있고 빌드 오류는 없다. `artifacts/ego-parity-v8-build.json`과 로그가 근거다. 앱 SHA-256은 `54f8dcfedc5b4f93e3128bfc74ba34221a3b50ca68d6f10997a76e34394819b2`, 번들 CLI는 `4824b50e344844d9d1c70cf62a1998fa9da680def9c98e49ea6259353ef50a7a`다. CLI help의 정상 JSON과 지원 안내를 확인했으며 브라우저 입력 검증으로 계산하지 않는다.

빌드 스크립트는 이전 번들을 교체하고 중간 파일 1,271개·표시 크기 683.2MiB를 정리했다. `artifacts/ego-parity-v8-postbuild.json`의 읽기 전용 후속 조회에서 이 프로젝트 dist 앱 PID와 임시 번들은 0개이며 target에는 최신 실행 파일 3개만 남았다. 서명은 local.lightbrowser.dev ad hoc이고 실행용 identity는 미지정이다. 기존 실행 보호를 유지해 앱·서버·검증 탭·작업을 시작하지 않았으며 키체인 재승인 질문과 보안 우회를 하지 않았다. 사용자 프로필과 저장 자료는 변경하지 않았다.

`docs/frame-pointer.md`를 추가하고 frame-input, frame-read, devtools-routing, iframe-control, pointer-input, memory, README, architecture, product-plan, preseller-integration, ego-parity-e2e를 갱신했다. E2E 08에는 중첩 OOPIF·border/padding/scale/zoom·부분 노출과 overlay, 큰 iframe의 스크롤, 이동 후 target/부모 변경·가림과 지원 밖 기하 거절을 추가했다. 최신 후보와 앱·CLI 해시를 v8로 고정하고 01·11은 blocked, 나머지는 pending으로 유지했다. 여러 프레임 본문을 합치는 공통 예산, 지원 밖 기하, focus 경합 예방, 최신 앱의 실제 CEF·사용자 화면 보존·로그인·IME·지속 메모리 검증은 남는다. 탭 복원·확대 보류와 독립판 다음 Preseller 내장 순서를 유지한다. 커밋·푸시·배포는 하지 않았다.

## 2026-10-18, 문서에 묶인 프레임 입력과 부모 검사, 통합 후보 v7

직전 v6의 본문 읽기에 이어 교차 출처 프레임의 참조와 입력 5개를 구현했다. goal은 active이며 Aside의 일상 사용성과 ego lite의 사람·AI 공간 및 CLI 제어를 결합한 독립 Naru 완성 목표를 유지한다. 사용자 요청대로 전체 회귀 묶음은 반복하지 않고 기능 연결과 한 번의 통합 빌드, 같은 바이너리의 E2E 목록 갱신을 진행했다. 이번 단계로 전체 목표를 완료 처리하지 않는다.

`src/browser.rs`의 read-frame은 문서·세대·unique context와 frame/session·Browser·소유권에 결합한 최신 snapshot을 탭마다 하나 보관한다. 최대 120개 공개 ref를 검증해 frame 접두사를 붙이고 focusedRef도 같은 집합에서만 반환한다. 기존 fill·insert·press·check·select가 이 참조를 받으면 현재 frame tree와 원 context를 다시 확인한다. 준비, Chromium 입력 배치, 사후 확인에 정확한 session과 unique context 및 DOM 바인딩을 유지하며 일반 check/select와 기존 DOM fill도 완료 시 같은 가드를 통과한다. 성공은 Runtime disable ACK 뒤 단일 응답으로 전송하고 취소·탐색·인계·늦은 응답은 기존 요청 수명 안에서 거절한다. click·hover는 frame ref에서 UNSUPPORTED_FRAME_POINTER로 거절한다. `src/cli.rs` 도움말에 지원 5개와 포인터 제한을 반영했다.

입력 전 부모 chain을 최대 32단계 확인한다. 각 부모의 고정 world에서 getFrameOwner와 resolveNode로 실제 iframe 노드를 얻고, 원 부모 unique context에서 원문서 소속·연결·숨김·inert·비활성 상태를 검사한다. objectGroup은 resolve 전 예약하고 함수 호출의 예외 객체에도 지정한다. 정상 경로는 group 해제 ACK 뒤 다음 부모로 진행하며 root 취소 경로는 group·Runtime·autoAttach 정리를 별도 ID와 ACK로 추적한다. child 객체는 session detach로 정리한다. 부모 좌표·clipping·가림과 지연 focus 변경의 예방은 이 검사에 포함하지 않는다.

`src/dom_bridge.js`의 readFrameActions는 WeakRef 요소와 WeakMap 문맥을 사용한다. frameAct는 binding을 먼저 검사해 오래된 요청이 새 관찰자를 지우지 않도록 하며 원문서와 읽기 세대가 다르면 입력하지 않는다. 부모 검사는 contentDocument·포커스·스크롤을 건드리지 않는다. 내부 clearFrame은 같은 세대의 명시적 해제를 제공하지만 native에서 호출하지 않으므로 이전 world의 메타데이터가 즉시 모두 해제된다고 설명하지 않는다. 임시 press·insert 관찰자는 소비·새 동작·읽기·최대 5초 단발 만료를 따르며 정지된 renderer의 타이머 지연도 가능하다. 새 프로세스, UI renderer, 저장소와 반복 타이머는 추가하지 않았다. 실제 RAM은 미측정이다.

고정 Chromium·V8의 session별 입력과 widget focus 선택, unique context와 object 인수 검증 및 원격 객체 수명 조사 22개를 `artifacts/frame-input-v1-sources.json`에 기록했다. 소스 검토에서 확정 차단 결함은 발견하지 못했고 callFunctionOn 예외 객체도 같은 group에 포함하도록 보강했다. 엔진 입력에는 frame 선택 인자가 없고 포커스 확인과 입력이 원자적이지 않으므로 이 검토로 잘못된 입력 예방이나 실제 CEF 성공을 주장하지 않는다.

cargo fmt, node --check src/dom_bridge.js, git diff --check와 문서의 로컬 링크 확인을 수행했다. 전체 Rust 단위, 모의 DOM, 로컬 CEF 회귀, 네이티브 UI, 외부 사이트와 E2E는 실행하지 않았다. 이전 바이너리의 통과 수를 새 소스에 적용하지 않았다. 문서 일괄 수정용 첫 Python 명령은 stdin 인코딩 SyntaxError로 쓰기 전에 종료됐으며 apply_patch와 ASCII로 인코딩한 데이터로 문서만 반영했다. 제품 빌드 실패나 런타임 실패와 구분한다.

`CARGO_NET_OFFLINE=true CARGO_BUILD_JOBS=2 ./scripts/dev.sh --release --build-only`를 한 번 실행했다. 세션 12194는 90.42초, exit 0이며 제품 소스와 Cargo 파일 34개의 빌드 전후 해시가 일치한다. 테스트 전용 registry의 remove·len·is_empty에 기존 dead_code 경고 한 묶음이 있었고 빌드 오류는 없다. `artifacts/ego-parity-v7-build.json`과 로그가 근거다. 앱 SHA-256은 `8d2ca086a412529a58460a88c7ff2d1a6c5b4d0a2dd8ec3a0425cdc3d08b8056`, 번들 CLI는 `fc985050a002bb50073d36688bc9ecd790a89603c2af6490d56171185669a35d`다. CLI help의 정상 JSON과 새 지원 안내만 확인했으며 페이지 실행 검증과 구분한다.

빌드 스크립트가 이전 번들을 교체하고 Cargo 중간 파일 1,271개·표시 크기 682.9MiB를 정리했다. 후속 읽기 전용 조회에서 이 프로젝트 dist 앱 PID와 임시 앱 번들은 0개이며 target에는 최신 실행 파일 3개가 남았다. 서명은 local.lightbrowser.dev ad hoc, 실행용 서명자는 미지정이다. `artifacts/ego-parity-v7-postbuild.json`에 해시·서명·도움말·정리를 기록했다. 앱·서버·검증 탭·작업을 실행하지 않았고 기존 프로필과 사용자 자료를 보존했다. 키체인 반복 승인 요청이나 보안 설정 우회는 하지 않았다.

`docs/frame-input.md`를 추가하고 README, architecture, product-plan, preseller-integration, frame-read, devtools-routing, iframe-control, memory, ego-parity-e2e를 갱신했다. E2E 08에는 원문서 입력 5개·root 및 실제 OOPIF·숨김 부모·오래된 ref·실패한 새 읽기·정리 중 취소·예외 객체·실제 입력 대상과 누적 자원 관측을 추가했다. 최신 후보와 앱·CLI 해시를 v7로 고정했으며 01·11의 blocked와 나머지 pending을 유지했다. 프레임 포인터 입력·부모 좌표/가림·다중 프레임 공통 예산, 지연 focus 경합 예방과 최신 실제 앱의 E2E는 남아 있다. 탭 복원·확대 보류를 유지하며 Preseller 내장은 다음 단계다. 커밋·푸시·배포는 하지 않았다.

## 2026-10-18, 프레임 본문 읽기와 context 수명, 통합 후보 v6

직전 goal turn의 하위 프레임 연결·v5 빌드 완료에 이어 교차 출처 프레임의 본문 읽기를 구현했다. goal은 active이며 독립 Naru 완성 뒤 같은 Rust·CEF 구현을 Preseller에 내장하는 목표를 유지한다. 사용자 요청대로 전체 회귀 묶음은 반복하지 않고 제품 구현, 한 번의 통합 빌드와 같은 바이너리의 E2E 목록 갱신을 진행했다.

`src/cli.rs`와 `src/protocol.rs`에 `read-frame TAB FRAME` 및 task run 범위 명령을 연결했다. `src/browser.rs`는 현재 inventory에서 대상 frame·session을 다시 찾고 Runtime disable ACK → enable replay → 고정 world 생성·재사용 → 고유 context 평가 → Runtime disable ACK 순서로 읽는다. `src/frame_sessions.rs`의 ContextTracker는 정확한 frame·world와 isolated 유형, isDefault=false, numeric ID와 uniqueId를 확인하며 파괴·초기화·교체 때 이전 읽기를 무효화한다. 원 Browser, session, 문서·탐색 세대, 작업 소유권과 token 검사를 유지하고 단계 사이 재진입에도 활성 inventory를 보존한다. 성공 응답의 요소 ref는 제거하고 focusedRef=null, actionsSupported=false, atomic=false로 반환한다. 프레임 입력을 지원한다고 표시하지 않는다.

`src/dom_bridge.js`의 readFrame은 현재 문서와 열린 Shadow DOM만 읽는다. 자식 iframe은 메타데이터만 반환하고 contentDocument를 조회하지 않는다. 기존 read 동작은 유지한다. 본문 12,000자·요소 120개·노드 10,000회·열린 root 64개·깊이 64·옵션 전체 240개 예산과 frame metadata 16개를 유지한다. Read 응답은 파싱 전 2MiB, 다른 프레임 단계는 256KiB다. 읽기 성공·예외 모두 finally에서 refs Map과 contexts WeakMap을 비워 노드·문서 연결을 해제한다. 고정 world를 재사용하지만 Runtime disable이 world나 이미 시작한 evaluate를 제거하는 것은 아니다. 새 UI renderer, 프로세스, 반복 타이머와 저장소는 추가하지 않았고 RAM 절감은 측정하지 않았다.

실패·인계·취소 때 child 연결을 해제하고 root Runtime을 활성화했을 가능성이 있으면 별도의 disable 요청을 보낸다. root Runtime과 auto-attach의 cleanup ACK를 각각 추적하며 정리 대기·실패를 성공으로 추정하지 않는다. 고정 Chromium·V8의 replay, 같은 이름 world 재사용과 disable 수명 조사 근거 13개를 `artifacts/frame-read-v1-sources.json`에 기록했다. 독립 소스 검토에서 추가 확정 차단 결함은 발견하지 못했지만 실제 callback 순서나 CEF 실행을 검증한 결과는 아니다.

cargo fmt --all --check, node --check src/dom_bridge.js, git diff --check는 통과했다. ContextTracker 단위검사 3개와 기존 CLI·protocol 검사 6개 안의 새 명령 자료는 준비만 했고 실행하지 않았다. 전체 Rust 단위, 모의 DOM, 로컬 CEF 회귀, 네이티브 UI, 외부 사이트와 E2E는 실행하지 않았다. 이전 바이너리의 통과 수를 이번 소스에 적용하지 않는다. `docs/frame-read.md`를 만들고 README, architecture, product-plan, preseller-integration, memory, iframe-control, devtools-routing, ego-parity-e2e를 갱신했다. E2E 08에는 문서별 본문 일치·자식 비재귀, world 재사용과 context 교체, 입력 참조 비반환, 취소·두 cleanup ACK 및 상한·지속 메모리 확인을 추가했다.

`CARGO_NET_OFFLINE=true CARGO_BUILD_JOBS=2 ./scripts/dev.sh --release --build-only`를 한 번 실행했다. 세션 29841은 89.88초, exit 0이며 제품 소스와 Cargo 파일 34개의 빌드 전후 해시가 일치한다. 테스트에서만 쓰는 registry의 remove·len·is_empty에 dead_code 경고 한 묶음이 있었으며 빌드 오류는 없다. `artifacts/ego-parity-v6-build.json`과 로그가 근거다. 앱 SHA-256은 `54bf8382e049a12d59d8b66c4745bbf31834dbfba812a3bc33ef0c1fc4d7ed8b`, 번들 CLI는 `23ed09d0341f132f6144032ac4448b1864963e33fef94b58a78cf7fd8acf4df1`다. CLI help의 정상 JSON과 read-frame 명령 표시를 확인했으며 브라우저 실행 검증과 구분한다.

빌드 스크립트는 이전 번들을 교체하고 Cargo 중간 파일 1,271개·표시 크기 682.5MiB를 정리했다. 읽기 전용 후속 조회에서 이 프로젝트 dist 앱 PID와 임시 앱 번들은 0개이며 target에는 최신 실행 파일 3개가 남았다. 서명은 local.lightbrowser.dev ad hoc, 실행용 서명자는 미지정이다. `artifacts/ego-parity-v6-postbuild.json`에 해시·서명·도움말과 정리를 기록했다. 앱·서버·검증 탭·작업을 새로 실행하지 않았고 기존 프로필과 사용자 자료를 보존했다. 키체인 반복 승인 질문과 보안 설정 우회는 하지 않았다.

교차 출처 입력용 ref와 frame-local 입력, 부모 좌표·가림 검사, 다중 프레임 본문의 전체 예산은 미구현이다. 지연 focus 경합 예방, 안정 서명과 최신 앱의 CEF·네이티브 UI·대표 서비스 로그인·IME·장시간 메모리 E2E도 남는다. 실행 보호와 대표 로그인 서비스 미정으로 E2E 01·11은 blocked, 나머지는 pending이다. 탭 복원·확대 보류를 유지하며 독립 브라우저나 Preseller 내장 완료로 보고하지 않는다. 커밋·푸시·배포는 하지 않았다.

## 2026-10-18, 하위 프레임 연결과 frames CLI, 통합 후보 v5

독립 Naru 완성이라는 active goal을 이어갔다. 사용자 요청대로 반복 전체 회귀 대신 구현과 통합 빌드, 같은 바이너리의 E2E 목록 준비를 진행했다. `src/protocol.rs`와 `src/cli.rs`에 frames TAB 및 task run 범위 명령을 추가하고 `src/browser.rs`에서 실제 Page.enable → iframe 필터의 Target.setAutoAttach → Page.getFrameTree로 연결했다. 각 child에도 준비와 조회를 수행하며 root ACK만으로 재귀 발견을 완료하지 않는다. 기존 창, 사용자 선택과 CLI를 유지하며 별도 UI renderer나 프로세스는 추가하지 않았다.

새 `src/frame_sessions.rs`는 부모가 알려진 iframe session만 최대 16개·깊이 8로 보관한다. 조회는 최대 17개 연결 방문(루트 포함), 전체 프레임 128개, 트리 깊이 32, URL 4,096·이름 256 UTF-8 bytes, 파싱 전 응답 256KiB·관심 이벤트 32KiB를 제한한다. 결과는 frameId, parentId, sessionId, URL과 이름뿐이며 atomic:false, contentIncluded:false다. 메타데이터 잘림은 truncated, 잘못된 연결이나 연결 한도는 FRAME_LIMIT, 중복 frame ID와 수집 중 변경은 FRAME_CHANGED로 구분한다. 준비한 연결은 다음 조회에서 재사용하고 결과와 방문 상태는 응답 뒤 해제한다.

`src/devtools_wire.rs`는 session별 직렬화와 빌린 Response/Event 파싱으로 확장했다. observer가 원 Browser·정확한 session·ID를 대조하고 루트 preview/capture/cleanup 응답과 분리한다. `src/lib.rs`에 browser 기능일 때만 새 모듈을 연결했다. 수집의 owner·scope·요청 token·탐색/문서 세대와 native getter 재진입을 검사한다. 소스 검토에서 pending 단계 사이에 이미 읽은 프레임의 변화가 누락될 수 있는 경계를 찾아 FrameControl.active_inventory와 invalidated 상태를 유지하도록 고쳤다. 재검토에서는 그 경계가 해소됐으며 추가 확정 결함을 찾지 못했지만 런타임 통과로 계산하지 않는다.

취소와 타임아웃, 인계·오류·렌더러 종료에서는 root autoAttach를 해제하고 실제 cleanup ACK를 기다린다. 연결 종료와 탭 닫힘에서는 상태를 제거한다. 해제 실패나 미응답을 성공으로 추정하지 않는다. 고정 Chromium 조사에서 getFrameTree가 renderer-local subtree만 반환하며 OOPIF 경계 노드를 제외함을 확인했다. root 해제는 자손을 폐기하지만 자손별 알림과 오류 응답은 모두 오지 않을 수 있어 부모 detach로 로컬 subtree를 함께 취소한다. 근거는 artifacts/frame-session-v1-sources.json이며 실제 CEF 관측과 구분한다.

전체 Rust 단위, 모의 DOM, 로컬 CEF 회귀와 실제 앱 E2E는 실행하지 않았다. Registry/정규화 경계 단위검사 7개, wire의 기존 2개 확장, CLI/protocol과 Browser 상관 검사 자료는 준비만 했다. cargo fmt와 git diff --check는 통과했다. README, architecture, product-plan, preseller-integration, memory, iframe-control, devtools-routing과 ego-parity-e2e 문서를 갱신했다. E2E 08에 단일·중첩·교차 site 프레임, 목록 재사용, 탐색 경쟁, 취소와 해제, 상한과 지속 메모리의 실제 확인 목록을 더했다.

CARGO_NET_OFFLINE=true CARGO_BUILD_JOBS=2 ./scripts/dev.sh --release --build-only를 한 번 실행했다. 세션 17826은 99.08초, exit 0으로 종료했으며 제품 소스/Cargo 파일 34개의 전후 해시가 일치한다. artifacts/ego-parity-v5-build.json과 로그가 근거다. 테스트에서만 사용하는 registry 메서드 remove·len·is_empty의 dead_code 경고 한 묶음이 있었으며 빌드 오류는 없었다. 앱 SHA-256은 `919272876a8a0d810d2a3f6627a86f30181e98bb90239a8fd955be2f01fc350a`, 번들 CLI는 `24539d7b5f4b05a14996ff5c79cd1c196be28583fc8cb9281ab790d4295b7347`다. 실제 브라우저를 시작하지 않는 CLI help의 정상 JSON과 frames TAB 표시는 확인했고 페이지 실행으로 계산하지 않았다.

번들은 local.lightbrowser.dev ad hoc이며 실행용 서명자는 미지정이다. 같은 날의 유효 서명자 조회를 반복하지 않았다. E2E 목록의 후보·앱/CLI 해시를 v5에 고정했다. 빌드 스크립트가 이전 번들을 교체하고 중간 파일 1,271개, 표시 크기 682.2MiB를 정리했다. 후속 읽기 조회에서 이 프로젝트 dist 앱 PID와 임시 번들은 0개이고 target에는 최신 실행 파일 3개만 남았다. artifacts/ego-parity-v5-postbuild.json에 서명 메타데이터·도움말·해시와 정리를 기록했다. 앱·서버·검증 작업이나 탭을 새로 실행하지 않았고 기존 프로필·사용자 자료는 변경하지 않았다.

교차 출처 frame-local 본문 읽기·입력, unique context와 자식 문서에 묶인 ref, 전체 읽기 예산 합산과 부모 좌표 검사는 미구현이다. 지연 focus 경합 예방, 대표 서비스 로그인·실제 IME·지속 메모리와 네이티브 UI 검증도 남는다. 키체인 반복 승인을 요청하거나 보안 설정을 우회하지 않았으며 안정 서명자 미지정으로 새 앱을 자동 실행하지 않았다. E2E 01·11의 blocked와 나머지 pending을 유지하고 독립 완성·Preseller 내장 완료를 주장하지 않는다. 탭 복원과 확대 보류는 유지하며 커밋·푸시·배포는 하지 않았다.

## 2026-10-18, DevTools raw 전송과 응답 상관관계, 통합 후보 v4

직전 goal turn은 insert 구현과 통합 빌드를 끝낸 progress다. 이번에는 교차 출처 iframe 연결에 필요한 전송 계층을 현재 명령에 적용했다. goal 조회는 active이며 독립 Naru 완성, Aside의 일상 사용성과 ego lite의 사용자 공간 및 AI 제어를 결합하는 목표를 유지한다. 프레임 목록만 조회하는 작은 별도 기능을 전체 프레임 제어의 완성으로 바꾸지 않았다.

`src/browser.rs`의 미리보기, 입력 배치, 입력 후 확인, 로딩 대기와 일반 명령 다섯 전송 지점을 공통 send_root_command로 연결했다. 예약한 ID를 raw JSON에 명시하고 SendDevToolsMessage에 전달한다. CEF Dictionary 중간 변환을 제거했으며 제출 접수와 실제 응답, 페이지 효과의 구분은 유지했다. 기존 unchecked ID 증가는 checked 발급으로 바꾸고 입력 배치의 checked 발급도 보존했다. ID 고갈 시 재사용하지 않으며 새 명령이나 미리보기 예약을 거절한다.

새 `src/devtools_wire.rs`는 양의 i32 ID, 정확히 하나의 result 또는 error 객체, 알려진 필드 중복과 sessionId 존재 여부를 검사한다. null이나 잘못된 타입의 sessionId도 루트로 해석하지 않는다. 필드 순서와 무관하게 원 payload 바이트를 빌려 넘기며 새 전체 Value 트리나 payload 재직렬화를 만들지 않는다. 기존 serde_json에 raw_value 기능만 켰고 버전과 새 의존성을 추가하지 않았다. Cargo.lock은 변경하지 않았다. src/lib.rs에 브라우저 구성에서만 모듈을 연결했다.

ControlObserver는 raw 응답을 전부 소비하고, ID가 가리키는 탭의 실제 Browser를 확인한 뒤 기존 완료 처리에 전달한다. 다른 Browser나 알 수 없는 하위 session 응답은 pending과 preview, capture를 제거하지 않는다. timeout이나 취소 뒤 남은 screenshot capture도 같은 Browser의 늦은 응답으로만 해제한다. typed callback은 제거해 중복 처리를 막았고 연결 종료도 현재 Browser를 확인한다. 이 단계는 루트 명령의 개선이며 child session registry, 지연 autoAttach, frame-local read와 입력은 아직 구현하지 않았다. 기존 작업 인계와 취소, 문서 검증, 입력의 재진입 가드와 캡처 수명을 유지한다.

기존 graphify 지도는 탐색 위치만 확인하고 실제 소스를 따라 검토했다. 고정 CEF의 typed parser가 sessionId를 전달하지 못하고 필드 순서를 가정하는 근거, SendDevToolsMessage와 ID 발급 차이는 artifacts/devtools-session-v1-sources.json에 기록했다. 현재 Naru에서 다른 탭이 잘못 조작되는 현상을 재현한 것은 아니다. 독립 소스 검토에서 추가 차단 결함을 찾지 못했지만 런타임 검증과 구분한다. 새 parser 경계 두 개와 Browser 상관관계 및 ID 고갈 한 개의 단위검사를 준비했으며 실행하지 않았다. 전체 Rust 단위, 모의 DOM, 로컬 CEF 회귀와 E2E는 실행하지 않았다. cargo fmt와 git diff --check만 통과했다.

CARGO_NET_OFFLINE=true CARGO_BUILD_JOBS=2 ./scripts/dev.sh --release --build-only를 한 번 실행했다. 실행 세션 20680은 87.18초, exit 0으로 종료했고 제품 소스와 Cargo 파일 33개의 전후 해시가 일치한다. artifacts/ego-parity-v4-build.json과 로그가 근거다. 앱 SHA-256은 `72c2011d98b1301dc405e1fcb25f48024a52f4473d1b56b72b771c951396c221`, 번들 CLI는 `43258e1238e0e0944cf234333a2d33c30633a28d352f8d3394058b042964881a`다. CLI help의 정상 JSON 응답만 확인했으며 브라우저 입력 검사로 계산하지 않았다. docs/ego-parity-e2e.md에 이 후보를 고정하고 read, screenshot, 입력과 취소, 복구 여정을 같은 앱으로 확인하도록 갱신했다. 연결 설계, iframe 계약의 오래된 합성 입력 설명, README, 아키텍처, 제품 계획, Preseller 내장과 메모리 문서도 갱신했다.

실행 서명자는 미지정이고 이번 번들도 local.lightbrowser.dev ad hoc이다. 앞선 같은 날 유효 서명자 0개 조회를 다시 반복하거나 키체인 승인 질문을 하지 않았다. 앱과 실제 CEF, 네이티브 UI, 외부 사이트, 로그인, IME와 지속 메모리 E2E는 미실행이다. E2E 01과 대표 서비스 미정인 11은 blocked이며 나머지는 pending이다. 하위 프레임 연결, 각 프레임 문서 수명과 공통 예산, 부모 좌표 검사, 지연 focus 경합 예방은 남아 있다. 보류한 탭 복원과 확대는 유지하며 Preseller 내장 완료도 주장하지 않는다.

빌드 스크립트가 이전 번들을 교체하고 중간 파일 1,270개, 표시 크기 681.4MiB를 정리했다. target에는 실행 파일 세 개만 있고 임시 앱 번들과 이 프로젝트 dist 앱 프로세스는 0개다. artifacts/ego-parity-v4-postbuild.json과 최종 소스 기록 ego-parity-v4-final.json에 남겼다. 사용자 프로필과 탭 자료는 변경하지 않았으며 앱, 서버나 검증 공간을 새로 실행하지 않았다. 추가 프로세스, renderer와 저장소 없이 기존 명령 상태와 빌린 payload를 사용하지만 실제 RAM 감소는 미측정이다. 커밋, 푸시와 배포는 하지 않았다.

## 2026-10-18, 현재 커서와 선택 영역에 넣는 insert 구현과 통합 후보 v3

독립 Naru를 먼저 완성하는 active goal을 이어갔다. 반복 전체 회귀검사 대신 제품 구현과 한 번의 통합 빌드, 최종 E2E 준비를 진행했다. `src/protocol.rs`와 `src/cli.rs`에 `insert TAB REF TEXT`와 작업 범위 명령을 추가했다. `src/browser.rs`는 기존 FillTarget과 Input.insertText 배치를 재사용하며 원래 Browser, 문서, 작업 제어권, isolated context와 임시 관찰 token을 사후 확인까지 유지한다. insert가 기존 합성 fill 경로로 들어가지 않게 구분했고 엔진 ACK만으로 성공하지 않는다.

`src/dom_bridge.js`는 입력칸의 값과 선택 범위, 편집 영역의 단일 Range를 포커스 전후 비교한다. 준비 과정에서 선택을 덮어쓰지 않으며 변하면 입력 전에 거절한다. 입력칸과 textarea는 삽입 뒤 예상 값이 같은지 확인한다. contenteditable은 원래 host의 trusted input, insertText와 요청 텍스트 일치를 관찰해 input-event로 반환하며 서식이나 문서 전체, 외부 편집기 모델을 보장하지 않는다. 비밀번호, 원래 값, 선택 본문과 내부 token은 응답에 넣지 않는다. 값 불일치, 원래 대상의 확인 불가와 다른 대상 입력을 구분하며 자동 재시도나 DOM fallback은 없다. 빈 문자열도 같은 엔진 경로이며 편집기에서는 이벤트 미확정일 수 있다.

추가 상태는 bridge마다 관찰 하나와 최대 5초 단발 정리, 제한된 문자열과 boolean이다. 새 프로세스, UI renderer, 반복 타이머와 저장 파일은 추가하지 않았다. 실제 메모리 절감은 측정하지 않았다. 준비와 엔진 입력 사이의 지연 focus 경합은 남으며 잘못된 입력의 예방이나 복구가 완료된 것으로 표시하지 않는다. 교차 출처 프레임의 공식 Chromium과 CEF 소스를 조사해 session별 연결, 문서 수명, 좌표와 공통 예산의 구현 경로를 `artifacts/cross-origin-frame-v1-sources.json`에 기록했다. 조사 결과는 기능 구현이나 런타임 검증이 아니다.

전체 Rust 단위와 모의 DOM, 로컬 CEF 회귀 묶음은 실행하지 않았다. cargo fmt, 제품 JavaScript 구문과 git diff --check를 통과했다. 공백 검사에서 발견한 docs/memory.md의 끝 빈 줄만 고친 뒤 다시 통과했다. 기존 CLI parser 검사에는 직접 명령, 작업 범위와 wire의 insert 자료를 추가했으나 실행하지 않았다. Rust와 DOM 연결의 읽기 검토에서 확정 차단 결함을 찾지 못했지만 실행 결과는 아니다. `tests/insert_text_smoke.py`와 `tests/fixtures/insert_text.html`에 커서 이어쓰기, Shift 선택 치환, textarea 여러 줄, plain/rich 편집기, 열린 shadow와 같은 출처 frame, 빈 값, 취소, 최대 길이, 대상 변경, 비밀번호 비노출과 사용자 선택·task revision 보존을 준비했다. 담당 에이전트가 Python AST, inline JavaScript 구문과 조사 JSON 구문만 확인했으며 실제 앱과 E2E는 실행하지 않았다.

`CARGO_NET_OFFLINE=true CARGO_BUILD_JOBS=2 ./scripts/dev.sh --release --build-only`를 한 번 실행했다. 같은 실행 세션 45446의 완료는 98.47초, exit 0이다. 제품 소스와 Cargo 파일 32개의 빌드 전후 해시가 일치한다. `artifacts/ego-parity-v3-build.json`과 로그가 근거다. 앱 SHA-256은 `92a6671440a399b6eed9d4c4f55ab6f8d1ba17f10a017aee3607538baf91b16d`, 번들 CLI는 `eea094fd8b506ddf73f61a291defd3be6872fc0af28590b9a6fcce139ee3d27d`다. 번들 CLI help에 insert가 포함된 것을 확인했으며 페이지 입력 실행과 구분한다. `docs/ego-parity-e2e.md`에 이 후보 해시와 입력 여정 06·07을 연결했다. 삽입 계약, README, 아키텍처, 제품 계획, 키 입력, Preseller 내장과 메모리 문서도 갱신했다.

이번 빌드도 실행 서명자는 미지정이고 번들은 local.lightbrowser.dev ad hoc이다. 같은 날 앞선 유효 identity 0개 조회를 다시 반복하지 않았으며 새 인증서, 키체인 설정과 실행 보호를 조작하지 않았다. 앱을 실행하지 않았고 보안 승인 질문도 하지 않았다. 최신 Naru CEF, 네이티브 UI, 외부 사이트와 로그인, 실제 IME 및 장시간 메모리 E2E는 미검증이다. E2E 01과 대표 로그인 서비스 미정인 11은 blocked이며 나머지는 pending이다. 교차 출처 iframe, 지연 포커스 경합 예방과 범용 편집기 등의 잔여 구현을 목표에서 제외하지 않았다. 탭 복원과 확대 보류, Preseller 내장을 독립판 이후에 진행하는 순서를 유지한다.

빌드가 이전 번들을 교체하고 Cargo 중간 파일 1,263개, 표시 크기 679.9MiB를 정리했다. target은 실행 파일 세 개만 남고 임시 앱 번들은 없다. 허용된 읽기 전용 ps 조회에서 이 프로젝트 dist 앱 프로세스는 0개다. 서명과 CLI help, 해시 및 정리 목록은 `artifacts/ego-parity-v3-postbuild.json`, 최종 소스와 준비 파일 기록은 `artifacts/ego-parity-v3-final.json`을 따른다. 앱과 서버, 검증 작업과 탭을 새로 만들지 않았으며 기존 프로필과 저장 자료를 보존했다. 커밋, 푸시와 배포는 하지 않았다. 독립 브라우저 전체 완료나 E2E 통과로 보고하지 않는다.

## 2026-10-18, hover와 macOS 편집 단축키를 통합 후보에 반영

사용자가 요청한 ego lite 수준의 독립 브라우저 완성 목표에 따라 제품 구현을 먼저 이어갔다. 이번 goal 조회는 active다. 앞선 기록의 paused는 당시 상태이며 현재 상태와 구분한다. 전체 회귀검사를 반복하지 않고 기능 구현, 통합 빌드, 실제 사용 여정의 E2E 준비 순서를 유지했다. 전체 목표나 실제 앱 검증을 완료로 표시하지 않았다.

`src/protocol.rs`와 `src/cli.rs`에 hover TAB REF를 추가하고 작업 범위 wire 허용 목록까지 연결했다. `src/browser.rs`는 기존 ClickTarget에 원래 action을 보관해 DOM 준비, Chromium mouseMoved 한 번, 같은 참조와 위치의 사후 확인까지만 수행한다. hover가 클릭 누름과 해제 배치를 만들지 못하게 분기했고 사후 확인 뒤 요청을 종료한다. `src/dom_bridge.js`는 같은 좌표, iframe과 shadow hit test를 재사용하며 hover 응답에도 원래 action을 유지한다. 성공 응답의 pointVerified는 위치 확인이며 메뉴 효과 완료는 후속 read로 확인해야 한다. 취소와 제어권 변경, 문서와 Browser 검사, attempted와 outcomeUnknown을 유지한다. 호출 흐름의 읽기 검토에서 확정 결함을 찾지 못했지만 실행 검증 결과는 아니다.

`src/keyboard.rs`에 macOS 편집 단축키 18개를 더해 지원 이름을 총 42개로 확장했다. Meta 전체 선택, 실행 취소와 다시 실행, 줄과 문서 경계 이동과 선택, Alt 단어 이동과 선택 및 삭제, Meta+Backspace를 포함한다. nativeVirtualKeyCode 0에서 네이티브 메뉴 처리를 추정하지 않고 keydown에 정적 Blink editing command 하나를 전달한다. keyup에는 command를 넣지 않는다. DOM 관찰은 원래 key와 code, Shift와 Alt 및 Meta, Ctrl false를 모두 대조한다. Meta+Delete, 클립보드와 임의 조합, IME는 추가하지 않았다. 공식 매핑과 조회의 한계는 `artifacts/mac-editing-commands-v1-sources.json`에 남겼다.

새 프로세스, 웹 UI renderer, 반복 타이머와 저장소는 추가하지 않았다. hover는 기존 제한된 포인터 상태를 재사용하고 키는 작은 KeyPress와 정적 command 문자열을 쓴다. 실제 메모리 감소나 장시간 회수는 측정하지 않았다. README, 포인터와 키 입력, 아키텍처, 제품 계획, Preseller 내장과 메모리 문서를 갱신했다. `docs/ego-parity-e2e.md`의 06과 07에 hover 메뉴 열기와 클릭 0회, 새 ref 선택, 편집 단축키의 실제 값과 선택 영역, preventDefault, 사용자 화면 보존을 반영했다.

전체 Rust 단위, 모의 DOM 회귀, 로컬 CEF 회귀, 독립 Chromium 검사와 실제 앱 E2E는 실행하지 않았다. cargo fmt, 제품 bridge의 node --check, git diff --check를 통과했다. CLI의 기존 허용과 거절 테스트 데이터는 새 지원 목록으로 맞췄으나 실행하지 않았다. `tests/native_click_smoke.py`와 fixture에 메뉴 열기부터 선택까지 한 흐름을 준비했고 Python AST와 inline JavaScript 구문 확인만 통과했다. `tests/native_press_smoke.py`와 fixture도 새 편집 조합 18개의 값과 선택 영역, native fill 뒤 Undo와 Redo, Meta+A와 Alt+Backspace 취소, 사용자 선택과 AI revision 보존을 확인하도록 준비했다. 이 두 파일 역시 Python AST와 inline JavaScript 구문 확인만 통과했고 실제 입력은 실행하지 않았다. 언어별 단어 경계와 범용 편집기의 undo 동작은 이 준비로 보장하지 않는다. 이전에 기록한 373개와 128개 등의 통과 수를 이번 소스에 적용하지 않는다. 이전 합성 contenteditable와 정확한 press 응답을 전제한 모의 DOM 기대 일부의 이행도 여전히 남아 있다.

`CARGO_NET_OFFLINE=true CARGO_BUILD_JOBS=2 ./scripts/dev.sh --release --build-only`를 한 번 실행했다. 통합 release 빌드는 86.91초, exit 0이며 제품 소스와 Cargo 파일 32개의 빌드 전후 해시가 일치했다. `artifacts/ego-parity-v2-build.json`과 로그가 근거다. 새 앱 SHA-256은 `74394d5490b15bdfb96029f94a3ea1f81b1ebccf7d07f47a901d1e716372bbcb`, 번들 CLI는 `19fa05c2a15a0d6b18d1192816cffa58c037da30f861d1f907b6a77cfa9be25a`다. 번들 CLI help의 hover와 편집 키 표시를 확인했고 이는 브라우저 입력 실행 검증과 구분한다.

서명은 local.lightbrowser.dev ad hoc이다. NARU_CODESIGN_IDENTITY는 미지정이며 읽기 전용 인증서 조회 결과 유효 identity 0개였다. `artifacts/ego-parity-v2-signing.json`에 기록했다. 기존 자동 실행 보호를 우회하거나 키체인 승인 질문을 다시 하지 않았다. 앱을 실행하지 않았으므로 최신 Naru의 CEF, 네이티브 UI, 외부 사이트, 로그인과 IME, 60분 지속 사용은 미검증이다. E2E 01과 대표 로그인 서비스가 미정인 11의 blocked, 나머지 pending을 유지한다. 지연 포커스 경합의 잘못된 입력 예방, 교차 출처 iframe과 범용 편집기 지원도 남아 있다. 탭 복원과 확대 보류는 유지한다.

빌드 스크립트는 이전 번들을 교체하고 Cargo 중간 파일 1,263개, 표시 크기 679.6MiB를 정리했다. target에는 실행 파일 세 개만 남았고 임시 앱 번들과 이 프로젝트 dist 앱 프로세스는 0개다. 최초 ps 조회는 sandbox PermissionError였고 허용된 읽기 전용 재조회로 확인했다. `artifacts/ego-parity-v2-postbuild.json`에 서명, 해시, 프로세스와 파일 목록을 남겼다. 앱과 서버, 검증 작업이나 탭을 새로 만들지 않았고 사용자 프로필과 저장 자료는 변경하지 않았다. 커밋, 푸시와 배포는 하지 않았다.

## 2026-10-16, ego lite 수준 목표와 구현 후 통합 E2E 방식으로 전환

사용자는 기본 개발판에서 멈추지 않고 ego lite 수준의 독립 브라우저를 완성하도록 목표를 명확히 했다. 반복적인 전체 회귀검사보다 남은 제품 구현을 먼저 진행하고, 통합 빌드 후 실제 앱의 E2E 목록으로 확인하라고 요청했다. `docs/ego-parity-e2e.md`에 14개 사용 여정과 같은 앱 해시 기준, pending, pass, fail, blocked 판정을 만들고 제품 계획과 Preseller 내장 계획을 연결했다. 이 목록은 최소 통합 관문이며 현재 미지원 기능을 자동으로 목표에서 제외하지 않는다. 사용자가 보류한 탭 복원과 확대는 유지한다.

앱 goal 조회 결과 기존 목표는 paused였다. 새 목표 등록은 unfinished goal이 있다는 오류로 거절됐다. 현재 goal 도구에는 재개나 목적 변경 기능이 없어 기존 목표를 완료 처리하거나 새 목표가 활성화됐다고 기록하지 않았다. 문서에는 변경한 목표와 개발 방식을 반영했고 사용자 요청에 따른 이번 구현은 진행했다. 최종 조회에서도 goal은 paused다.

`src/browser.rs`는 PressTarget에 원래 키와 참조, isolated context와 관찰 token을 연결한다. 두 키 입력 응답 뒤 PressVerify로 이어지며 같은 탭의 pending을 유지한다. `src/dom_bridge.js`는 최대 하나의 passive keydown 관찰을 등록하고 원래 대상과 다른 요소로 전달된 관찰을 구분한다. Rust는 미일치에 KEY_TARGET_MISMATCH와 attempted, outcomeUnknown을 반환한다. 관찰이 없는 경우 unconfirmed이며 Enter 탐색이 ACK 직후 또는 사후 확인 중 발생한 경우에도 같은 의미를 유지한다. 제어권 상실, 취소, Browser 교체와 렌더러 종료 오류는 유지한다. 키 이벤트를 차단하거나 페이지 효과를 되돌리지 않으므로 지연 포커스 경합 자체는 해결된 것으로 판정하지 않는다.

명시적 contenteditable fill은 기존 합성 beforeinput과 직접 자식 교체에서 전체 Range 선택, Chromium Input.insertText, native innerText 비교로 바꿨다. 준비에서 선택과 포커스를 확인하고 사후 읽기 전에 노드 10,000개, 깊이 64와 텍스트 상한을 확인한다. 요청과 결과의 줄바꿈을 임의로 정규화하지 않는다. 새 renderer나 상주 프로세스는 없으며 키 관찰은 새 읽기와 동작, 소비 또는 5초 단발 만료에서 정리한다. renderer가 멈추면 실제 만료 정리 시각은 지연될 수 있다. 키 입력, 일반 입력칸, contenteditable, 아키텍처와 메모리 문서, README를 갱신했다.

검증은 사용자 요청에 따라 구분했다. 전체 Rust 단위, 모의 DOM 회귀와 CEF 회귀 묶음은 실행하지 않았다. cargo fmt, JavaScript 구문과 공백 검사는 통과했다. 기존 Rust 키 검사 연결은 새 사후 확인 계약으로 조정했지만 실행하지 않았다. 기존 모의 DOM의 합성 contenteditable와 정확한 이전 press 응답을 전제로 한 일부 기대는 새 계약으로 이행할 잔여 작업이며 이전 373개와 128개의 통과를 이번 코드에 적용하지 않는다. E2E용 tests/contenteditable_smoke.py와 fixture는 단일 줄, 여러 줄, 빈 값, shadow와 frame, 취소와 읽기 전용, 대상 변경과 인계 흐름으로 조정했다. tests/native_fill_smoke.py의 이전 synthetic 편집기 분기도 정리했다. Python AST 2개 확인은 exit 0이며 담당 에이전트 출력 bf0403이 근거다. 실제 E2E는 실행하지 않았다.

`CARGO_NET_OFFLINE=true CARGO_BUILD_JOBS=2 ./scripts/dev.sh --release --build-only`로 통합 release 빌드를 한 번 실행했다. 전체 과정은 106.78초, exit 0이며 소스와 Cargo 파일 32개가 빌드 전후 일치한다. 근거는 `artifacts/ego-parity-v1-build.json`과 로그다. 새 앱 실행 파일 SHA-256은 `71989690515d8630669d1053f137306ae229a13dc0740ec7614d86fc0f1d111c`, 번들 내부 CLI는 `4e1e1e2a9243489ea407e9e1cd33f48ed37bb9aaa6bac5ccf9be1794bcc1149f`다. 후보 해시를 E2E 목록에 고정했으며 서명은 local.lightbrowser.dev ad hoc이다.

실제 Naru CEF, 네이티브 UI, 외부 사이트, 로그인과 IME, 장시간 메모리 E2E는 미실행이다. NARU_CODESIGN_IDENTITY가 미지정이고 읽기 전용 유효 서명자 조회가 일반 환경과 허용 환경 모두 0개였다. 기존 dev.sh는 이 조건의 자동 실행을 빌드 전에 거절한다. 사용자에게 키체인 승인을 다시 요청하거나 보안 설정을 우회하지 않았다. E2E 01은 blocked, 대표 로그인 서비스가 미정인 11도 blocked이며 나머지는 pending이다. 빌드 성공을 앱 동작 성공으로 기록하지 않는다.

빌드 스크립트가 Cargo 중간 파일 1,263개, 표시 크기 679.5MiB를 정리했다. 최종 target에는 실행 파일 3개만 남고 임시 앱 번들도 없다. 프로세스 조회는 최초 sandbox에서 ps 실행이 차단됐으며 허용된 읽기 전용 재조회로 이 프로젝트 dist 앱의 PID 0개를 확인했다. `artifacts/ego-parity-v1-postbuild.json`이 근거다. 새 앱과 서버를 실행하지 않았고 기존 프로필과 탭 데이터는 변경하지 않았다. 새 커밋과 푸시, 배포는 없다.

## 2026-10-16, 키보드 확장 이후 ego lite 대비 진행 상태 확인

사용자의 진행 상태 질문에 따라 실제 앱 비교 기록, 제품 계획, 내장 계획과 최신 검사 원본을 대조했다. 비교 기준은 2026-10-11에 직접 확인한 ego lite의 공간과 CLI 사용 흐름이며 최신 전체 기능 동등성을 평가한 것은 아니다. 현재는 사용자와 AI 공간, CLI와 일상 브라우저 기능을 갖춘 개발용 알파다. 최근 Chromium 클릭, 텍스트 입력과 24개 키 형식은 소스 구현 상태이며 기존 앱에 반영된 기능과 구분한다.

`artifacts/keys-native-v1-rust-tests.json`의 소스 및 Cargo 파일 32개와 보존 실행 파일 2개의 해시가 현재 파일과 모두 일치했다. 해당 기록의 Rust 단위 373개 통과와 모의 DOM 기록의 128개 통과를 확인했으며 이번에 검사를 다시 실행하지 않았다. 독립 Chromium의 제품 bridge 검사는 28개 중 27개 통과이고, 첫 입력의 지연된 포커스 변경으로 다른 입력칸에 키가 전달된 실패 1개는 남아 있다. 별도 포커스 조건의 준비 거절 검사 통과를 원래 실패의 해결로 합산하지 않는다.

직전 포커스 조사에서는 고정 Chromium 소스를 읽었다. focus emulation은 페이지의 포커스뿐 아니라 가시성과 이벤트, 해제 수명에도 영향을 주므로 해결책으로 적용하지 않았다. 근거는 `artifacts/focus-delivery-v1-sources.json`이다. 이번 상태 확인에서 제품 코드를 추가로 변경하지 않았다.

최신 Naru CEF와 네이티브 UI의 통합, 외부 사이트의 로그인과 복잡한 입력, 실제 IME, 장시간 자원 회수와 같은 조건의 메모리 비교, 빌드 사이의 안정적인 서명이 남는다. Preseller 내장은 설계 단계이고 탭 복원 보류는 유지한다. 독립 Chromium 관찰과 이전 Naru 앱의 성공을 최신 앱 검증으로 계산하지 않았다.

상태 확인으로 변경한 추적 파일은 이 작업 기록뿐이다. 새 빌드, 앱이나 서버 실행, 서명과 키체인 조작은 없으며 정리할 새 빌드 생성물도 없다. 기존 앱과 사용자 프로필, 탭 데이터를 보존했고 커밋과 푸시는 하지 않았다.

## 2026-10-16, AI 키보드 이동과 현재 포커스 참조

직전에는 ego lite 대비 상태를 확인하고 앞선 입력 작업의 검사 생성물과 기록을 정리했다. 이번에는 독립 브라우저의 웹 폼과 드롭다운 조작을 위해 Enter 전용 CLI를 확장했다. `src/keyboard.rs`의 작은 KeyPress 값을 CLI와 브라우저가 공유하며 기본 키 14개와 지정한 Shift 조합 10개를 지원한다. `src/lib.rs`, `src/cli.rs`, `src/browser.rs`를 연결하고 기존 프로토콜의 key 문자열 형식은 유지했다. 임의 문자와 미지원 조합은 CLI와 서버에서 포커스 준비 전에 거절한다.

`ActionEvidence::Press`가 원래 키를 보관하고 준비 응답이 다른 허용 키로 바뀌어도 거절한다. 기존 Chromium 입력 배치를 재사용해 누름과 해제 두 요청, 접수와 응답 집계, TAB_BUSY, 취소와 제어권 및 문서 검사, 늦은 응답 차단을 유지한다. Enter와 Space만 텍스트 있는 keyDown이고 나머지는 rawKeyDown이다. nativeVirtualKeyCode는 0이며 Shift는 두 이벤트의 modifier 8이다. 별도 modifier 이벤트나 지속적인 키 상태, editing commands와 자동 재시도는 추가하지 않았다. 기본 이동과 삭제, 선택, 취소는 Chromium이 처리한다.

`src/dom_bridge.js`는 같은 24개 키의 포커스 준비를 지원한다. `read.focusedRef`는 이번 interactive 목록에 포함된 가장 안쪽 논리 포커스 참조이며 기존 문서, 열린 root, frame과 깊이 상한을 사용한다. 읽기 세대와 ref, 두 포커스 관측을 확인하고 Rust에서 같은 반환 목록과 대조한 뒤 탐색 세대 접두사를 붙인다. 키 입력 후 다시 read해서 다음 대상으로 사용할 수 있다. 각 press는 지정 ref에 포커스를 요청하므로 같은 원래 ref에 Tab을 반복하는 동작은 연속 순회가 아니다. 새 영속 저장, 전체 문서 재순회, 프로세스나 UI renderer는 추가하지 않았다. 실제 메모리 절감량은 측정하지 않았다.

공개 DOM에서는 닫힌 Shadow DOM 내부 포커스와 직접 host 포커스를 구별할 수 없었다. 열린 root가 없는 shadow host 가능 요소는 div와 일부 contenteditable을 포함해 null로 반환한다. 접근 불가 프레임, 반환 참조와 순회 상한 밖, 관찰 중 변경이나 예외도 null이다. 이것은 포커스 부재 확정값이나 OS 창 활성 상태가 아니다. read는 focus 호출이나 창 활성화를 하지 않는다.

검증 근거는 다음과 같이 구분한다.

- format, 오프라인 lib 테스트 컴파일, 기본 구성과 CLI 전용 구성의 전체 타깃 검사가 모두 exit 0이었다. 근거는 `artifacts/keys-native-v1-check.json`과 로그다. 같은 실행 파일의 Rust 단위는 `373 passed; 0 failed; 0 ignored`였고 테스트 출력 기준 1.84초였다. 새 검사 5개와 기존 수명 회귀 4개 확장으로 canonical 키 파싱, 실제 전송 파라미터, 원 키 치환 거절, raw 요청 거절, read 결과의 focusedRef 재사용을 확인했다. 근거는 `artifacts/keys-native-v1-rust-tests.json`이다. 자체 임시 Unix 소켓 검사가 가능한 허용 환경에서 실행했고 CEF는 초기화하지 않았다.
- 모의 DOM은 128개 통과다. 지원 키와 준비 무부작용, readonly, alias 거절, 열린 shadow와 iframe의 포커스, 참조와 순회 상한, 예외와 문서 변경 및 재진입을 다룬다. 담당 에이전트의 실제 `node --test` exit 0 출력 chunk d30561과 최종 구문 및 해시 출력 chunk 7c8fc1을 `artifacts/keys-native-v1-dom-summary.json`에 명시했다. 원본 TAP 로그 파일을 새로 만든 것처럼 기록하지 않았다.
- 독립 ego Chromium의 사전 관찰 46개와 추가 7개, closed shadow 관찰 6개는 제품 통과 수에 합산하지 않는다. 포커스된 div의 PageUp, PageDown과 Space에서 스크롤 효과가 없었던 관찰도 보존했다. 최종 제품 bridge와 fixture로 검사한 28개 중 27개가 통과했고, 준비 거절을 기대한 redirect 1개가 실패했다. 문서가 포커스되지 않아 준비 단계에서는 핸들러가 실행되지 않았고, CDP 입력의 활성화 뒤 다른 textarea로 이동해 Enter가 그곳에 전달된 실제 경합이다. 같은 제품 소스에서 이미 포커스된 문서의 준비만 따로 검사한 1개는 FOCUS_NOT_REACHED, attempted false, 엔진 전송 0개였다. 이를 원래 실패 해결이나 28개 일괄 통과로 재분류하지 않는다. 원본과 집계는 `artifacts/keys-native-v1-final-probe.json`, `keys-native-v1-focused-rejection.json`, `keys-native-v1-browser-summary.json`에 있다.
- 독립 Chromium은 152이며 Naru의 CEF 156과 다르다. focus 에뮬레이션은 쓰지 않았지만 ego CDP의 자동 활성화가 있어 Naru의 배경 선택 보존과 AppKit 동작 증거가 아니다. `tests/native_press_smoke.py`, `tests/fixtures/native_press.html`, `tests/smoke.py`에 24개 키와 취소, focusedRef 후속 검사를 준비했다. Python 3개와 fixture inline JS 구문만 확인했으며 실제 Naru smoke는 실행하지 않았다. 구문 근거는 `artifacts/keys-native-v1-smoke-static.json`이다. 최신 앱 CEF, 네이티브 UI, 외부 사이트, IME, 첫 요소와 마지막 요소의 Tab 경계, 장시간 RAM과 자원 회수는 남는다.

키 매핑은 고정 Chromium의 Input.pdl, keyboard_codes_posix.h, editing_behavior.cc를 확인했다. 처음 잘못된 editor_behavior.cc 경로의 404도 `artifacts/keys-native-v1-sources.json`에 보존하고 공식 디렉터리 목록에서 실제 파일명을 확인해 가져왔다. README, 아키텍처, 키 입력 계약, 제품 계획, 메모리와 Preseller 내장 계획을 갱신했다. 독립 최종 소스 검토에서 추가 차단 결함은 발견하지 못했다. 최종 해시와 문서 링크, 공백 및 전용 target 부재 확인은 `artifacts/keys-native-v1-final-static.json`에 기록했다. 독립 브라우저 완성과 Preseller 내장은 여전히 진행 중이며 이번 입력 확장을 전체 완성으로 판정하지 않는다.

컴파일 세션 80812와 단위 세션 22019는 exit 0으로 종료됐다. 이번 전용 `target/.keys-native-test-6_cpy3__`의 경로와 inode를 확인한 뒤 파일 2,060개와 디렉터리 290개를 정리했다. 파일 크기 합계는 1,509,108,608바이트이며 물리 디스크 회수량은 아니다. symlink는 없었고 삭제 전후 소스와 보존한 앱, CLI 해시가 일치했다. `artifacts/keys-native-v1-processes.json`과 `artifacts/keys-native-v1-cleanup.json`이 근거다. ego TaskSpace 12는 finish 한 번으로 p1과 함께 종료했고 unmanaged 탭 0개였다. 업데이트 알림은 확인했지만 업그레이드하지 않았다. 새 Naru 앱, 서버, 서명과 키체인 조작은 없었다. 사용자 프로필과 기존 탭은 보존했으며 커밋과 푸시는 하지 않았다.

## 2026-10-16, Chromium 텍스트 입력 연결과 ego lite 대비 상태 갱신

독립 브라우저 목표를 이어 일반 텍스트 입력을 Chromium에 연결했다. 이후 사용자의 진행 상태 질문에 따라 현재 소스와 검사 원본, 2026-10-11 실제 앱 비교 기록을 다시 대조하고 남아 있던 검사 생성물과 기록을 정리했다. 질문 이후 제품 코드를 추가로 바꾸거나 앱을 빌드하지 않았다. 아래 결과는 직전 구현과 완료된 검사에 대한 기록이며 질문에 답하면서 새로 실행한 제품 검사로 계산하지 않는다.

`src/browser.rs`와 `src/dom_bridge.js`의 fill은 text, search, tel, url, password와 textarea에서 전체 선택 준비, `Input.insertText` 한 번, 원래 대상의 값 확인 순서다. 빈 문자열 삭제도 같은 경로를 사용한다. Chromium의 beforeinput 취소와 maxlength를 따르며 최종 값이 요청 문자열과 같아야 성공한다. 준비부터 사후 확인까지 원 Browser, 문서, 참조와 제어권을 보존하고 대기 잠금을 유지한다. 전달 응답만으로 성공하지 않으며 입력 시도 뒤 자동 재시도하지 않는다. email, number, 날짜 등 나머지 지원 유형과 contenteditable은 기존 DOM 처리를 유지한다. 응답에는 원문이나 실제 입력값, 임의 페이지 예외를 넣지 않는다.

포커스가 준비와 전송 사이에 바뀔 가능성은 남는다. Chromium 입력은 당시 포커스를 대상으로 하므로 사후 확인이 잘못 전달된 입력을 되돌리지 않는다. CRLF 정규화도 엄격한 문자열 비교에서는 미도달로 처리한다. 실제 하드웨어 입력, 모든 편집기 호환성, 입력 도중 다른 페이지 조작과의 원자성을 보장하지 않는다. 요청 동안의 원문과 Browser 참조 외에 새 상주 프로세스, renderer나 영속 저장은 추가하지 않았다. 실제 RAM 절감량은 측정하지 않았다.

검증 근거는 다음과 같이 구분한다.

- Rust 단위는 `368 passed; 0 failed; 0 ignored`였다. 새 회귀 8개는 입력 준비, 단일 전송, 사후 확인, 원 대상과 제어권, 취소와 늦은 응답, 오류의 정보 제한을 다룬다. `artifacts/text-native-v1-rust-tests.json`과 로그가 근거다. 자체 임시 Unix 소켓 사용이 허용된 환경에서 같은 테스트 실행 파일을 검사했으며 CEF 엔진은 초기화하지 않았다.
- format, 오프라인 잠금 의존성 lib 테스트 컴파일, 기본 구성과 CLI 전용 구성의 전체 타깃 검사가 모두 exit 0이었다. 기존 test 전용 dead_code 경고 3개가 남았다. `artifacts/text-native-v1-check.json`에 기록했고 소스와 Cargo 파일 31개, 보존한 앱과 CLI 2개의 해시가 전후 일치했다.
- 모의 DOM은 116개가 통과했다. `artifacts/text-native-v1-dom-after.json`이 근거다. 독립 ego lite Chromium에서는 입력 종류 40개와 선택 6개를 관찰했으며 이를 제품 통과 수로 세지 않는다. 최종 bridge 검사는 29개 중 28개가 통과했고 own getter fixture의 setter 누락으로 1개가 실패했다. fixture만 고쳐 같은 제품 소스로 해당 1개를 통과해 총 29개 계약을 확인했다. 전체 29개를 다시 일괄 실행한 결과는 아니다. 집계는 `artifacts/text-native-v1-summary.json`을 따른다.
- 독립 검사는 Chromium 152이며 Naru가 고정한 CEF Chromium 156과 다르다. 포커스 에뮬레이션은 쓰지 않았으나 ego CDP의 대상 활성화가 있으므로 Naru의 배경 선택 보존이나 AppKit 입력 증거로 설명하지 않는다. 기본 제한 환경의 XPC 시작 실패는 로그에 보존했고 원인을 확인한 뒤 허용된 실행 환경에서 같은 공간을 이어 검사했다. TaskSpace 11은 finish 한 번으로 종료했으며 p1 종료와 closedSpace true를 확인했다. 업데이트 알림은 전달했지만 업그레이드하지 않았다.
- `tests/native_fill_smoke.py`와 `tests/fixtures/native_fill.html`에 Naru 통합 검사 항목을 준비했다. 관련 Python 4개와 inline JavaScript 문법을 확인한 근거는 `artifacts/text-native-v1-final-smoke-static.json`이다. 이 smoke를 Naru에서 실행하지 않았다. 독립 검토에서 추가 차단 결함을 찾지 못했지만 최신 Naru CEF 전송, 네이티브 UI, 외부 로그인 사이트, IME와 장시간 자원 검증은 남는다.

입력 계약, README, 아키텍처, 제품 계획, Preseller 내장 계획과 프로젝트 메모리를 갱신했다. ego lite에서 관찰한 공간과 CLI 협업 구조는 구현했으며 일상 탐색 기능도 갖췄지만 현재 판정은 개발용 알파다. 일반 키와 IME, 교차 출처 iframe, 내장 모델과 자율 실행기, 안정적인 개발 서명과 동일 앱의 통합 검증, 장시간 메모리 측정이 남는다. Preseller 내장은 설계 단계이고 재시작 탭 복원 보류를 유지한다.

컴파일 세션 85682와 단위 세션 98601의 exit 0 종료 결과를 기록했다. 이번 전용 `target/.text-native-test-_0n0d8wx`의 경로와 inode를 확인한 뒤 파일 2,060개, 디렉터리 290개를 정리했다. 파일 크기 합계는 1,507,351,048바이트이며 물리 디스크 회수량은 아니다. symlink는 없었고 삭제 전후 소스와 기존 실행 파일 해시가 일치했다. 근거는 `artifacts/text-native-v1-processes.json`과 `artifacts/text-native-v1-cleanup.json`이다. 새 Naru 앱, 서버, 서명과 키체인 조작은 없었으며 사용자 프로필과 기존 탭 데이터를 보존했다. HEAD는 7074841이며 이번 후속 변경은 커밋하거나 푸시하지 않았다.

## 2026-10-16, ego lite 기준 진행 상태 재확인

사용자의 진행 상태 질문에 따라 제품 계획, 2026-10-11 실제 앱 비교 기록, 현재 CLI와 작업 제어 경로, 최신 검증 원본을 대조했다. 기존 graphify 지도는 구조 탐색에만 사용했다. 현재 소스와 Cargo 파일 31개의 SHA-256이 `artifacts/cli-observation-v1-rust-tests.json`의 검사 대상과 전부 일치하며, 해당 원본은 Rust 단위 360개 통과다. 이번에 단위 검사를 다시 실행한 것은 아니다.

사용자와 AI 공간, 작업 생성과 인계, 중지와 종료, 기본 CLI와 일상 탐색 기능을 갖춘 개발용 알파로 판단한다. 클릭과 Enter의 Chromium 입력 전환은 소스 구현과 독립 Chromium 근거가 있지만 최신 Naru CEF 실행은 남았다. fill, select, check는 DOM 경로이며 일반 키, IME, 교차 출처 iframe, 내장 모델 연결과 자율 실행기는 미구현이다. Preseller 내장은 설계 단계다. 최신 앱 하나로 로그인, 입력, 다운로드, 사용자 개입과 중지를 이어 검증해야 하며 장시간 메모리와 복구, 안정적인 개발 서명도 남은 과제다. 과거 Naru CEF 결과나 독립 ego 실행을 최신 Naru 앱 검증으로 합산하지 않았다. 탭 복원 보류는 유지한다.

직전 시작한 Input.insertText 조사는 기존 fill의 입력 종류와 값 정규화, 포커스 경합에 관한 소스 검토까지만 진행했다. 제품 구현이나 입력 fixture 검증은 없으므로 완료 기능에 포함하지 않는다. 검증 공간 10은 `finish({keep:[]})` 한 번으로 종료했고 실제 응답은 closedSpace true, p1 종료, 보존할 unmanaged 탭 0개였다. 처음 증거 파일 저장은 ego Node의 상대 경로에서 ENOENT로 실패했지만 공간 종료는 성공했다. 종료 호출을 반복하지 않고 실제 명령 출력의 응답을 `artifacts/text-input-v1-cleanup-space.json`에 보존했다. ego lite 업데이트 알림을 확인했으며 업그레이드는 실행하지 않았다.

이번 제품 코드 변경, 새 단위 검사, 모의 DOM, Naru CEF, 네이티브 UI와 외부 사이트 검사는 없다. 앱 빌드, 서명, 앱이나 서버 실행도 없으며 새 빌드 생성물은 없다. 기존 앱, 사용자 프로필과 탭 데이터를 정리 대상으로 삼지 않았다. 현재 HEAD는 7074841이며 로컬 후속 변경을 새로 커밋하거나 푸시하지 않았다. 변경한 추적 파일은 이 작업 기록뿐이다.

## 2026-10-16, CLI 관측과 로딩 대기 수정 완료, 단위 검증

직전 진행 상태 답변은 기존 기능과 남은 일을 점검한 것이며 구현 진전은 없었다. 목표를 이어 현재 소스를 다시 확인하고 중단했던 CLI 상태 관측 수정을 마무리했다. `overview()` 메서드를 필드처럼 읽던 컴파일 오류도 해소했다. 독립 브라우저 완성과 같은 구현의 Preseller 내장 목표는 여전히 진행 중이며 이번 변경을 커밋하거나 푸시하지 않았다.

`src/browser.rs`의 탭 목록, Open 생성 응답, renderer와 로딩 진단, 대기 결과는 공통 관측 경로를 사용한다. 원 Browser와 탐색, 문서, 로딩 세대 및 소유권을 잡은 뒤 STATE borrow 밖에서 CEF 값을 읽는다. getter 전후와 최종 JSON 조립 전에 상태를 재검사하며 `tab_info`는 네이티브 호출 없는 직렬화만 담당한다. 실제 main-frame URL과 요청 URL의 의미를 구분한다. 작업 범위의 목록은 최신 scope와 전체 대상 집합을 확인하고 범위 없는 목록은 모든 탭의 메타데이터를 볼 수 있다. 목록 일부만 성공으로 반환하지 않는다. 탭 생성 뒤 조회만 실패하면 같은 오류에 `tabCreated:true`, 생성된 `tab`, `stateUnavailable:true`, `outcomeUnknown:false`를 넣어 중복 생성으로 이어지지 않도록 구분한다.

대기는 관측 전에 제거하지 않는다. tab, 시작 시각, 활성 플래그 Arc로 원래 대기를 식별하고 오래된 tick, probe 오류, DOM 응답과 예약 실패가 교체된 대기를 건드리지 않게 했다. 관측이 불안정해도 기존 절대 마감은 유지한다. 원래 탐색 실패나 renderer 종료는 별도 오류로 반환하며, 상태를 읽지 못했으면 `state:null`, `stateUnavailable:true`로 표시한다. 준비 상태 확인은 대기당 하나이고 최종 응답은 기존 Reply 활성 플래그로 한 번만 전달한다.

ReadyState의 각 CDP 전송은 host와 파라미터 준비 뒤 원 Browser, 문서, 소유권, scope, 대기 식별과 마감을 확인한다. 내부 상태에 등록한 뒤 다른 네이티브 조회 없이 전송한다. callback도 원 Browser와 일치해야 최종 관측을 시작하고 완료할 수 있다. 일반 페이지 변경 명령 전체를 재설계하거나 자동 재시도를 추가하지 않았다. 탐색은 URL과 히스토리 및 로딩 확인, 권한 대기와 찾기 정리, 최종 관측, begin, 즉시 전송 순서다. 정리 중 대상이 바뀌면 전송하지 않은 탐색 의도를 남기지 않는다. 사람이 중지된 AI 탭을 탐색하는 권한과 CLI scope 검사는 분리하며, CLI 배경 탐색과 frame 없는 renderer 복구 Reload를 유지한다.

검증은 다음과 같이 구분한다.

- `cargo fmt --all --check`, 오프라인 잠금 의존성 lib 테스트 실행 파일 컴파일, 기본 구성과 CLI 전용 구성의 전체 타깃 검사가 모두 exit 0이었다. 근거는 `artifacts/cli-observation-v1-check.json`과 해당 로그다. 기존 test 전용 dead_code 경고 3개는 앞선 검사와 문구 및 개수가 같았다.
- 컴파일이 만든 같은 해시의 실행 파일로 Rust 단위 `360 passed; 0 failed; 0 ignored`를 확인했다. 테스트 출력 기준 1.84초다. 기존 소켓 검사에 필요한 임시 Unix 소켓을 허용한 환경에서 실행했으며 Naru 앱이나 CEF 엔진을 초기화한 검사가 아니다. 근거는 `artifacts/cli-observation-v1-rust-tests.json`과 로그다. 소스 및 Cargo 파일 31개와 기존 앱, CLI 2개의 해시는 컴파일과 단위 검사 전후 일치했다.
- 새 회귀 10개는 getter 5개 경계에서의 탭 제거, Browser와 문서 교체, 로딩과 소유권 변경, 실제 URL 구분, 목록 전체 재검사, Open의 생성 사실, 절대 마감과 오류 상태, 대기 교체 중 관측과 전송 및 예약, 원 Browser callback, CDP 준비 중 취소, 네이티브와 CLI 권한, 정리 뒤 탐색 차단을 다룬다. 기존 ReadyState fixture 3개는 시작 시각 식별자를 공유하도록 조정했다. 실제 함수에 조회와 전송 경계를 주입한 상태 검사이며 실제 CEF 재진입 증거로 설명하지 않는다.
- 독립 소스 검토에서 최종 helper 연결과 신규 회귀에 추가 차단 결함을 발견하지 못했다. 모의 DOM, 독립 Chromium, 최신 Naru CEF, 네이티브 UI와 외부 사이트 검사는 이번에 실행하지 않았다. 느린 HTTP, redirect, 로그인과 사용자 인계, 종료 중 callback, 장시간 RAM과 실제 네이티브 참조 회수는 남은 검증이다.

`docs/cli-state-observation.md`, README, 아키텍처, 제품 계획, 메모리와 화면 상태 읽기 문서를 갱신했다. 조회 동안 대상 Browser와 Frame, 실제 URL을 잠시 보관하고 목록의 비용은 대상 탭 수에 비례한다. 기존 대기 타이머를 재사용하며 새 worker, renderer, 반복 타이머나 영속 저장은 없다. RAM이나 응답 속도 절감량을 측정한 결과는 아니다.

컴파일 세션 82953과 단위 세션 32935는 모두 exit 0으로 종료됐다. 전용 `target/.cli-observation-test-6ohvsee0`의 경로와 inode를 확인한 뒤 파일 2,060개, 디렉터리 290개를 정리했다. 파일 크기 합계는 1,505,035,583바이트이며 물리 디스크 회수량은 아니다. symlink는 없었고 삭제 전후 소스와 기존 실행 파일 해시가 일치했다. `artifacts/cli-observation-v1-processes.json`, `artifacts/cli-observation-v1-cleanup.json`에 근거를 남겼다. 앱 번들 교체, 앱이나 서버 실행, 서명과 키체인 조작은 없었으며 사용자 프로필과 기존 탭 데이터를 변경하지 않았다.

최종 정적 확인에서도 소스와 기존 실행 파일 해시 33개가 일치했고 변경 문서의 로컬 링크 123개에 누락이 없었다. 공백 검사도 통과했다. `artifacts/cli-observation-v1-final-static.json`이 근거이며 HEAD는 `7074841`, 새 커밋과 푸시는 없다.

## 2026-10-16, ego lite 대비 현재 개발 단계 점검

사용자의 진행 상태 질문에 따라 2026-10-11 실제 비교 기록, 제품 계획, 현재 코드와 검증 원본을 읽었다. ego lite 기준은 당시 확인한 버전 0.5.1.13의 사용자 공간, CLI 작업 공간, 배경 선택 유지와 실제 페이지 입력이다. 이번에는 ego lite를 다시 실행하지 않았으며 전체 기능 동등성이나 최신 버전 비교를 수행한 것은 아니다. 기존 graphify 구조 지도는 호출 경로를 찾는 데만 쓰고 현재 소스를 별도로 확인했다.

현재 단계는 사용자와 AI의 협업 구조, 기본 CLI 조작과 일상 브라우저 기능을 구현한 개발용 알파다. 과거 Naru 실행에서 공간과 선택 보존, 일부 사용자 인계, 북마크, 검색, 다운로드와 찾기를 확인한 기록이 있다. 이후 추가한 방문 기록, 탭 정렬, iframe과 입력 보강 등은 소스 검사와 실제 앱 검증을 구분해야 한다. 내장 대화와 모델 연결은 없으며 Preseller의 부모 뷰와 수명주기를 받는 공통 연결 API도 아직 없다. Rust를 썼다는 이유로 ego lite보다 가볍다고 결론 내릴 수 없고 동일 조건의 자원 비교와 장시간 회수 검증이 남아 있다.

`native-focus-v3-suite.json`의 과거 로컬 CEF 32개 exit 0과 `shell-snapshot-v1-permitted-rust-tests.json`의 앞선 소스 단위 350개 통과를 다시 확인했다. 현재 `src/browser.rs` 해시는 후자의 검사 대상과 다르며, 미완성 CLI 관측 수정에는 `overview` 메서드를 필드처럼 사용하는 코드와 대기 수명 관련 미완료 범위가 남아 있다. 따라서 현재 작업 폴더를 최종 빌드 가능한 완료본으로 보고하지 않는다. 원격 커밋 7074841, 그 이후의 로컬 변경, 기존 앱 번들을 구분한다.

제품 코드는 변경하지 않았고 이번 기록만 추가했다. 새 단위 검사, 모의 DOM, CEF 통합, 네이티브 UI와 외부 사이트 검사는 실행하지 않았다. 빌드나 앱, 서버를 시작하지 않아 새 실행 프로세스와 빌드 생성물 정리는 없다. 우선 미완성 수정을 끝낸 뒤 최신 소스를 하나의 앱으로 검증하고, 실제 로그인과 입력, 다운로드, 사용자 인계 및 중지 흐름을 확인해야 한다. 키체인 재승인 문제, 복잡한 페이지 호환성, 장시간 자원 사용은 남은 과제이며 재시작 탭 복원은 사용자의 보류 결정을 유지한다.

## 2026-10-16, 기존 커밋의 비공개 원격 푸시 재확인

사용자의 “푸시까지 해야지” 요청에 따라 기존 `codex/navigation-readiness` 브랜치의 커밋을 푸시했다. GitHub 조회에서 `FEABE/naru-browser`의 비공개 상태를 확인했고, 로컬 HEAD와 원격 작업 브랜치가 모두 `707484185feaa0cc14bff75c81c8cca18b87e76d`임을 확인했다. `git push origin HEAD:refs/heads/codex/navigation-readiness`는 exit 0과 `Everything up-to-date`를 반환했다. 기본 브랜치 main은 변경하지 않았다. 첫 제한 환경 네트워크 조회는 이름 해석과 연결 오류로 실패했고, 허용된 네트워크 환경에서 다시 조회해 확인했다.

이후 로컬에서 진행하던 변경은 스테이징하거나 새로 커밋하지 않았다. 완료된 Reload/Stop과 화면 상태 조회 변경 외에 `src/browser.rs`의 CLI 상태 관측 작업이 미완성 상태로 섞여 있다. 구현 담당자의 쓰기를 중지하고 그대로 보존했다. 상태 관측 helper, Tabs/Open과 navigate 전환 일부만 저장됐으며, `overview()` 호출 수정 두 곳, waiter 식별과 ReadyState 전송 경합 보호, 회귀 검사가 남아 있다. `docs/cli-state-observation.md`도 아직 검증 전 초안이다. 이전 350개 단위 검사 통과는 앞선 소스에 대한 결과이며 현재 미완성 소스의 통과 결과로 재사용하지 않는다.

이번 작업의 변경은 이 기록뿐이다. GitHub 비공개 상태, 원격 SHA와 푸시 명령 결과를 확인했으며 단위 검사, 모의 DOM, 로컬 CEF, 네이티브 UI와 외부 사이트 검사는 실행하지 않았다. 새 빌드, 앱이나 서버 실행, 서명과 키체인 조작이 없어 정리할 실행 프로세스와 빌드 생성물은 없다. 사용자 프로필과 기존 탭 데이터는 변경하지 않았다. 전체 브라우저 완성과 Preseller 내장은 여전히 진행 중이다.

## 2026-10-16, 화면 상태 조회의 borrow 분리와 갱신 보류

직전 목표 턴은 Reload/Stop 의도와 주소 초안 보존 소스, Rust 345개 검사와 생성물 정리를 마친 진전이다. 이번에는 Naru의 사용자 공간과 AI 배경 탭을 함께 표시하는 상태 조회 경계를 보완했다. 독립 브라우저 완성과 같은 구현의 Preseller 내장 목표는 유지하며 이번 변경은 커밋하거나 푸시하지 않았다.

`src/browser.rs`의 기존 `shell_snapshot`은 STATE borrow 안에서 CEF 로딩과 뒤로가기, 앞으로가기 getter를 호출했다. 소스에서 확인한 borrow 충돌 가능성이며 실제 앱에서 충돌을 재현한 결과는 아니다. 현재 구현은 Browser와 탐색, 문서, 로딩 관측 세대를 캡처하고 borrow 밖에서 조회한다. getter 전후와 모든 탭 조회 후에 원래 관측을 재검사한다. 뒤쪽 탭의 조회가 앞선 탭을 바꾸거나 새 탭을 만들면 전체 스냅샷을 보류한다. 최종 STATE에서 최신 제목, 공간별 순서와 선택, AI 소유권을 조립한다. 로딩 시작과 완료 콜백은 탐색 세대와 별개인 관측 세대를 올린다.

중첩 조회는 Cell과 RAII로 차단한다. 구현 중 검토에서 `then_some(Self)`의 eager 생성이 실패한 취득에서도 Drop을 실행해 바깥 가드를 해제하는 오류를 찾아 명시적 분기로 고쳤다. 연속 두 번의 중첩 조회와 정상 종료 후 재취득을 회귀에 포함했다. 소스가 동결되기 전에 수정했으며 실패한 컴파일이나 앱 실행 결과로 기록하지 않는다.

`src/control_panel.rs`의 여섯 호출부는 `Option<ShellSnapshot>`을 처리한다. 무효 결과를 빈 탭 목록으로 바꾸지 않고 기존 화면을 유지하며 브라우저의 기존 100ms 갱신 예약을 사용한다. 다운로드 사이트 조회는 같은 대상의 카드를 지우지 않도록 Skipped로 처리한다. 선택 확인이 실패하면 후속 포커스 이동을 중단한다. refresh의 창 표시와 최소화 조회도 SHELL borrow 밖으로 옮겼다.

통합 검토에서 첫 sync가 보류되면 캐시된 이전 탐색 세대 때문에 Stop 주소 초안이 다시 만료되는 연결 회귀를 찾았다. 명령 결과 직후에 opaque NavigationAddressReceipt를 잡고 sync 뒤에 내부 상태만으로 확인하도록 수정했다. 원래 Browser, 문서, 제어권, 탭과 공간, URL, 결과 세대를 확인하며 로딩 관측만 바뀌거나 무관한 배경 탭이 닫힌 경우는 허용한다. UI는 기존 초안의 메타데이터가 그대로인 경우에만 기준 세대를 옮기고 문자열을 덮지 않는다. 무효 receipt에는 이전 오류 안내도 표시하지 않는다. 보존 상태는 동기 호출을 넘겨 유지하지 않는다.

검증은 다음과 같이 구분한다.

- `cargo fmt --all --check`, 오프라인 잠금 의존성의 lib 테스트 실행 파일 컴파일, 기본 구성과 CLI 전용 구성의 전체 타깃 검사가 모두 통과했다. 근거는 `artifacts/shell-snapshot-v1-check.json`과 해당 로그다. 소스와 Cargo 파일 31개, 기존 앱과 CLI 2개의 해시가 검사 전후 일치했다.
- 첫 제한 환경 단위 검사는 `347 passed; 3 failed`였다. 실패는 기존 protocol 소켓 검사 3개의 `bind socket: Operation not permitted (os error 1)`이며, 신규 상태 조회 검사 실패가 아니었다. 원본은 `shell-snapshot-v1-rust-tests.json`과 로그에 보존했다. 테스트가 자체 임시 폴더에 Unix 소켓을 만들고 정리하는 경로임을 확인한 뒤, 소켓 사용이 허용된 환경에서 같은 해시의 실행 파일을 재실행했다. 최종 결과는 `350 passed; 0 failed; 0 ignored`, 3.23초이며 근거는 `shell-snapshot-v1-permitted-rust-tests.json`과 로그다. 이를 Naru 앱 실행 결과로 설명하지 않는다.
- 신규 Rust 회귀 5개는 getter 중 mutable STATE 접근, 최신 선택과 소유권 투영, 각 getter 경계의 닫힘과 Browser 교체, 탐색과 문서 변경, rejected 탭, 로딩 시작과 완료를 다룬다. 앞선 관측의 무효화, 새 탭 누락 방지, 중첩 취득과 해제도 포함한다. 실제 `shell_snapshot_with`에서 로딩 콜백이나 배경 탭 제거로 None을 만든 뒤 주소 receipt의 보존과 만료를 검사한다. 기존 UI 순수 검사 2개는 오래된 표시 세대와 유효하거나 무효한 receipt 경계에 맞춰 갱신했다. CEF getter를 주입한 상태 검사이며 실제 CEF 재진입이나 AppKit 입력 검사는 아니다.
- 독립 소스 검토에서 최종 snapshot, 여섯 UI 호출부와 receipt 연결에 추가 차단 결함을 발견하지 못했다. 모의 DOM, 독립 Chromium, 실제 Naru CEF, 네이티브 UI, 외부 사이트와 장시간 RAM 검사는 이번에 실행하지 않았다. 기존 바이너리나 과거 검사를 최신 소스의 앱 검증으로 대신하지 않는다.

`docs/shell-snapshot.md`를 추가하고 README, 아키텍처, 제품 계획, 메모리와 Reload/Stop 문서를 갱신했다. 조회 한 번의 임시 관측은 현재 탭 수에 비례하며, 주소 증표는 Browser 참조 하나와 기존 가드 및 URL을 호출 동안만 보관한다. 영속 캐시, 새 worker, 반복 타이머, 창이나 renderer를 추가하지 않았다. 실제 메모리 감소량을 측정한 것은 아니다. CLI의 `tab_info`와 탐색 시작, 스냅샷 반환 이후 전체 AppKit 렌더의 재진입 경계는 별도 후속 작업이다.

컴파일 세션 35918은 exit 0, 첫 단위 세션 44962는 exit 101, 최종 단위 세션 27416은 exit 0으로 종료를 확인했다. 별도 ps 조회는 샌드박스에서 차단됐으며 프로세스 종료 판단에는 도구의 세션 종료 결과를 사용했다. 전용 `target/.shell-snapshot-test-1127dj9v`의 경로와 inode를 확인한 뒤 파일 2,060개, 디렉터리 290개를 삭제했다. 파일 크기 합계는 1,499,556,252바이트이며 물리 디스크 회수량이나 RAM 측정값은 아니다. 심볼릭 링크는 없었고 삭제 전후 소스와 기존 앱 및 CLI 해시가 일치했다. `artifacts/shell-snapshot-v1-cleanup.json`에 결과를 기록했다. 새 Naru 앱, CEF 초기화, 외부 HTTP 서버, 서명과 키체인 조작은 없었고 사용자 프로필과 기존 탭 데이터는 건드리지 않았다.

최종 정적 확인에서 변경 문서의 로컬 링크 117개에 누락이 없었고 `git diff --check`가 통과했다. 소스와 기존 실행 파일 33개 해시도 다시 일치했다. lib 테스트 컴파일 경고는 기존 dead_code 3개와 같았다. 최종 HEAD는 `7074841`이며 새 커밋과 푸시는 없다. 근거는 `artifacts/shell-snapshot-v1-final-static.json`이다.

## 2026-10-16, 새로고침과 로딩 중지의 의도 및 주소 초안 보존

직전 턴은 클릭 변경을 `7074841`로 커밋하고 비공개 원격 SHA 일치까지 확인한 진전이다. 독립 브라우저 목표를 이어 로딩 중지 표시가 실행 단계에서 새로고침으로 바뀌는 경로와 주소 초안 소실을 보완했다. 이번 추가 변경은 커밋하거나 푸시하지 않았다. Naru 전체 완성과 같은 구현의 Preseller 내장은 미완료다.

`src/browser.rs`는 표시한 탭, 공간, 탐색 세대와 중지 여부를 `NavigationControlTarget`으로 받는다. CEF 로딩 상태, provisional과 intent_pending을 함께 확인한다. 완료된 로딩과 진행 중인 같은 중지의 중복 요청은 인계, 정리와 전송 없이 끝낸다. 과거 stopped 상태가 남아 있어도 이후 새 리소스 로딩은 중지할 수 있다. 새로고침을 선택한 뒤 새 로딩이 시작됐다면 중지로 바꾸지 않고 거절한다. 오류 화면의 다시 시도도 명시적인 새로고침 경로를 사용한다.

네이티브 조회와 정리 콜백 전후 원래 Browser, 문서, 탐색 세대와 작업 소유권, 현재 공간과 선택을 검사한다. 필요한 AI 인계가 만든 정확한 revision과 epoch 증가만 허용한다. AppKit 조회는 마지막 CEF 로딩 표본보다 먼저 하고 이후에는 순수 상태만 검사한다. 처음 구현을 검토하면서 표본 뒤 AppKit 조회가 로딩을 완료시키면 오래된 표본으로 AI 작업을 멈출 수 있는 경계를 찾아 이 순서를 바꿨다. CLI stop은 별도 경로에서 로딩이 없어도 권한 대기, 찾기와 방문 후보 정리 후 stop_load를 보내는 기존 계약을 유지한다. 배경 선택을 바꾸지 않고 전송 전 요청 취소와 대상 변경은 거절한다.

`src/control_panel.rs`는 버튼, 메뉴와 ⌘R을 같은 경로로 연결했다. 기존 ⌘⇧R도 같은 해석을 유지하며 강제 새로고침 기능은 추가하지 않았다. 메뉴는 열릴 때의 대상을 보관하고 검색, 북마크, 다운로드와 공간 목록, 입력기 조합 상황에서는 실행을 막는다. 동기 호출 문맥 한 개로 주소 자동 갱신을 잠시 보류한다. 주소 텍스트를 저장해 되쓰지 않으며 최신 sync 후 런타임이 확인한 epoch에만 원래 초안의 기준을 맞춘다. 검토 중 중지 이후 epoch를 아직 반영하지 않은 화면으로 보정하면 항상 실패하는 경계를 확인해 sync와 보정 순서를 연결했다.

첫 구현의 341개 검사가 통과한 뒤 AI 인계 직후 창 포커스 변화로 명령이 실패하면 E와 E+1 차이로 초안이 만료되는 조건부 소스 경로를 확인했다. 실제 UI 재현 결과는 아니다. 최종 구현은 실패 결과에도 원래 Browser, 문서, 소유권과 선택, 예상 epoch가 유지된 경우만 보존 근거를 함께 반환한다. Stop과 전송 전에 실패한 Reload 모두 적용하며 성공 Reload에는 보존 예외를 주지 않는다. 명령 실행 가능 여부와 주소 보존 가능 여부를 분리해 같은 페이지의 overlay나 창 포커스 변화가 입력 내용을 버리지 않게 했다. 실제 새 문서, 다른 소유권과 공간, 바뀐 초안 메타데이터에는 보정하지 않는다. 오류를 성공으로 표시하거나 자동으로 반복하지 않는다.

README, 아키텍처, 제품 계획, 메모리 문서와 새 `docs/reload-stop.md`에 계약과 한계를 기록했다. 아키텍처와 Preseller 계획에 남아 있던 클릭 전체를 합성 입력으로 설명한 문장도 이미 커밋한 Chromium 클릭과 Enter 소스에 맞췄다. 독립 Chromium 관찰을 최신 Naru 통합 성공으로 바꾸지는 않았다. 새 저장소, 상주 프로세스, renderer와 반복 타이머는 추가하지 않았다.

검증은 다음과 같이 구분한다.

- v1 Rust 단위 341개와 format, 오프라인 잠금 의존성 lib 테스트 컴파일, 기본 구성과 CLI 전용 구성의 전체 타깃 검사가 통과했다. `artifacts/reload-intent-v1-check.json`, `reload-intent-v1-rust-tests.json`에 당시 소스와 결과를 보존했다. 이후 확인한 실패 초안 경계는 이 결과만으로 완료했다고 판정하지 않았다.
- 최종 v2 Rust 단위 345개가 전부 통과했다. 기존 330개에 런타임 회귀 10개와 UI 상태 회귀 5개를 추가했다. 실제 상태 전환 함수에 네이티브 조회와 전송 경계를 주입해 중지 무동작, 중복 중지, 인계, 전송 횟수, 새 로딩 거절, Browser와 문서 교체, 소유권 변화, 실패 이후 초안 보존과 만료를 확인했다. UI 검사는 표시 대상과 주소 메타데이터 함수의 순수 검사이며 실제 AppKit 클릭 검사가 아니다.
- v2의 format, lib 테스트 컴파일, 기본 구성과 CLI 전용 구성의 전체 타깃 검사도 통과했다. `artifacts/reload-intent-v2-check.json`, `reload-intent-v2-rust-tests.json`이 근거다. test 전용 dead_code 경고 3개와 CLI 전용 lib 9개, lib test 1개는 v1과 문구, 개수가 같고 출력 순서만 일부 달랐다. `reload-intent-v2-warnings.json`에 비교 결과를 기록했다.
- 이번에는 모의 DOM, 독립 Chromium, 실제 Naru CEF 통합, 네이티브 UI와 외부 사이트 검사를 실행하지 않았다. DOM bridge 변경은 없고 앱 번들, release 바이너리와 서명을 새로 만들지 않았다. 키체인 승인 질문이나 조작 없이 소스와 상태 회귀 검사로 제한했다. 느린 HTTP 응답, 실제 메뉴와 주소 편집, IME, renderer 복구와 장시간 자원 회수는 후속 검증이다.

기존 전체 shell snapshot의 CEF getter 호출 수명은 이 변경의 guard 검사와 별도 검토 대상이다. 이번에 stop 경로의 STATE borrow 안에서 네이티브 getter를 호출하던 부분을 제거했지만 브라우저 전체의 재진입 검사가 끝났다는 뜻은 아니다. 새로운 native command를 전송한 뒤 CEF가 실제 로딩을 멈췄는지도 단위 검사로 입증하지 않는다.

컴파일 세션 62910, 14115와 단위 검사 세션 35901, 3730은 모두 exit 0으로 종료됐다. v2는 v1의 같은 임시 target을 재사용했다. 마지막에 `target/.reload-intent-test-hfqzt9we`의 identity와 symlink 0개를 확인한 뒤 파일 2,353개, 크기 합계 1,674,374,878바이트를 정리했다. 이 합계는 물리 디스크 회수량이나 RAM 측정값이 아니다. 최종 소스 31개와 기존 앱, CLI 2개의 해시는 컴파일, 검사와 정리 전후 일치했다. `artifacts/reload-intent-v2-cleanup.json`에 결과를 기록했다. 이번 작업에서 Naru 앱, HTTP 서버나 브라우저 검증 공간은 실행하지 않았다.

최종 공백 검사와 변경 문서의 로컬 링크 111개 존재 검사를 통과했다. 소스와 기존 바이너리 해시 33개가 검증 기록과 일치하며 임시 target 부재를 확인했다. `artifacts/reload-intent-v2-final-static.json`에 기록했고 HEAD는 `7074841`, 이번 9개 변경 파일은 미커밋 상태다.

## 2026-10-16, Chromium 클릭 변경의 커밋과 비공개 푸시 요청

사용자의 커밋 요청과 이어진 푸시 요청에 따라 CLI 클릭 구현, 테스트, 문서의 15개 파일을 기존 `codex/navigation-readiness` 브랜치에 커밋하고 같은 원격 브랜치로 올리는 범위다. 새로고침과 로딩 중지 보완은 설계만 진행한 상태에서 멈췄으며 이번 커밋에 포함하지 않는다. 제품 코드를 추가로 수정하지 않았고 아래 구현 기록의 미커밋 표시는 당시 상태다.

GitHub 조회에서 `FEABE/naru-browser`의 `isPrivate: true`를 확인했다. 작업 전 로컬 HEAD와 원격 작업 브랜치는 모두 `d3a0d8b28783530652679b7d390924521d7ff702`이었다. 제한 환경의 첫 네트워크 조회는 실패했으며 네트워크 접근을 허용한 조회는 성공했다. 푸시 대상은 기본 브랜치 main이 아닌 기존 작업 브랜치다.

기존 검사 기록에는 Rust 단위 330개, 모의 DOM 106개, 독립 Chromium의 고유 계약 32개 통과와 기본 구성, CLI 전용 구성 검사 결과가 있다. 커밋 전 현재 소스와 검사 파일의 해시를 이 기록과 대조하며 동일 소스의 빌드와 실행 검사를 반복하지 않는다. 최신 Naru CEF 통합, 네이티브 UI와 배경 선택 보존, 외부 사이트와 장시간 RAM 검증은 여전히 미완료다.

이번 작업에서 앱, 서버와 빌드를 실행하지 않는다. 빌드 산출물, 사용자 프로필과 검증 원본은 Git 제외 상태를 유지한다. 해시 대조, 공백 검사, 기존 임시 target 정리 상태, 커밋과 푸시의 실제 결과, 최종 로컬과 원격 SHA, 작업 트리는 `artifacts/commit-push-native-click-20261016.json`에 기록한다.

## 2026-10-16, CLI 클릭을 Chromium 포인터 입력으로 연결

직전 턴은 Enter 변경을 `d3a0d8b`로 커밋하고 비공개 원격 SHA 일치를 확인한 진전이다. 독립 브라우저 목표를 이어 이번에는 합성 DOM click을 Chromium 포인터 입력으로 바꿨다. 이번 변경은 커밋하거나 푸시하지 않았으며 Naru 완성과 같은 구현의 Preseller 내장 목표는 여전히 미완료다.

`src/dom_bridge.js`는 현재 ref, 문서와 활성 상태를 검사하고 viewport 안의 후보를 hit test한다. 대상 자체와 비대화형 자식은 허용하고 다른 대화형 자식은 거절한다. 필요할 때 대상과 부모 iframe을 한 번씩 스크롤하고 다시 계산한다. 검사 한 번의 후보는 최대 9개, iframe은 최대 8단계, shadow와 조상 탐색은 64단계다. 같은 출처 iframe의 border, padding과 검증한 양수 배율을 반영하고 계산한 박스 크기를 실제 크기와 대조한다. 독립 검사에서 display: contents의 individual scale을 곱하면 잘못된 좌표가 나온 것을 확인해 박스 변환은 제외하고 실제 적용되는 zoom은 유지했다. SVG 조상, motion path와 지원하지 않는 프레임 변환은 명시적으로 거절한다. quirks viewport는 body의 client 크기를 사용한다.

`src/browser.rs`는 기존 Enter 배치를 공통 입력 배치로 바꿨다. 포인터 이동 한 요청의 접수와 응답 뒤 원래 isolated context, ref와 같은 좌표를 DOM에서 다시 확인한다. 이 재검사는 스크롤하거나 새 점을 고르지 않는다. 통과하면 같은 호스트에 왼쪽 mousePressed와 mouseReleased를 차례로 접수한다. 단계마다 기존 pending을 교체해 TAB_BUSY를 유지하고 네이티브 getter 뒤와 제출 직전에 활성 요청, 제어권, Browser와 문서 세대를 재검사한다. 이동부터 attempted가 true이며 이후 DOM 거절이나 누름 전 취소에도 보존한다. 두 입력의 접수 반환값과 응답을 확인한 최종 결과는 dispatchAcknowledged만 표현한다. 자동 반복과 DOM click fallback은 없고 check, fill, select의 기존 의미는 유지한다.

`src/cli.rs`의 도움말을 바꾸고 `tests/fixtures/native_click.html`, `tests/native_click_smoke.py`를 준비했다. 기본 smoke와 shadow smoke는 새 ACK 계약과 기존 페이지 효과를 함께 확인하도록 연결했다. 다운로드 권한 smoke도 새 계약을 검사하지만 과거 합성 click의 통과 결과를 재사용하지 않는다. 사용자 활성화에 따라 권한 순서가 달라질 수 있어 실제 앱에서 다시 확인해야 한다. README, 아키텍처, 제품 계획, 메모리 문서와 새 `docs/pointer-input.md`에 현재 구현과 한계를 반영했다. 입력 배치는 최대 두 pending을 유지하며 새 프로세스, renderer, 반복 타이머와 저장소는 없다.

검증은 다음과 같이 구분한다.

- 모의 DOM 최종 106개가 전부 통과했다. 기존 94개와 새 12개를 포함하며 위치 준비, 스크롤, 가림, 다른 대화형 자식, SVG 대상, shadow와 슬롯, iframe 변환, verifyOnly의 같은 점 검사와 참조 변경을 다룬다. 최종 bridge SHA-256은 `d7cdb500230ad645440c5cfc089464724b890d77b6084b140a04de39ec90ff0e`다. `artifacts/pointer-native-v1-dom-after.log`와 JSON에 명령, 결과와 해시를 보존했다.
- Rust 단위 330개가 전부 통과했다. 기존 Enter 회귀 8개를 유지하고 클릭 회귀 6개를 추가했다. 실제 reserve, pending 교체와 응답 처리를 stub 전송으로 실행해 이동 ACK 뒤 접수 실패, 잘못된 준비 결과, 변경된 좌표, hover 거절, 문서와 제어권 변경, 취소 후 attempted, 단계 사이 TAB_BUSY를 확인한다. CEF 엔진 실행 검사는 아니다. 오프라인 잠금 의존성 단위 실행 파일 컴파일, 기본 구성과 CLI 전용 구성의 전체 타깃 검사, format도 통과했다. `artifacts/pointer-native-v1-check.json`, `pointer-native-v1-rust-tests.json`이 근거다. 기본 구성의 test 전용 dead_code 경고 3개, CLI 전용 lib 경고 9개와 lib test 경고 1개를 확인했으며 경고 문구는 직전 Enter 검사와 같다.
- 독립 ego Chromium 152의 자체 공간 9에서 최종 bridge를 isolated world에 넣고 main world fixture와 연결했다. 기본 동작 8개, geometry 18개, 추가 경계 6개의 고유 계약 32개를 확인했다. 실제 pointer, mouse, click과 checkbox 기본 동작을 ACK와 따로 관찰했다. 추가 경계의 첫 실행은 4개 통과, 2개 실패였으며 문서 observer가 검사 사이 누적되어 같은 이벤트를 여러 번 기록한 검사기 문제였다. 원본을 보존하고 AbortController로 리스너를 정리한 뒤 해당 두 사례만 다시 실행해 통과했다. 제품 소스는 바꾸지 않았다. `artifacts/pointer-native-v1-browser-summary.json`과 연결된 원본이 근거다. BackCompat의 오른쪽 아래 fixed 대상도 클릭했지만 이 실행에서는 root와 body의 client 크기가 같았으므로 둘의 차이로 생기는 결함까지 직접 재현한 증거는 아니다.
- 앞선 좌표 조사 11개와 추가 CSS 경계 7개에서 display: contents, SVG와 motion path의 계산 차이를 관찰했다. 이를 최종 소스의 통과 개수에 합산하지 않는다. 설치된 CEF의 Chromium 156.0.8078.17 소스에서 좌표 배율, 페이지 포커스와 소비 여부를 보장하지 않는 ACK 경로를 확인했다. `artifacts/pointer-native-v1-primary.json`에 URL과 해시를 기록했다. Chromium 152 실행과 Chromium 156 소스 확인은 서로 다른 증거다.
- Python AST 4개와 새 fixture의 JavaScript 구문 1개가 통과했다. `artifacts/pointer-native-v1-integration-static.json`에 파일 해시를 기록했다. 준비한 Naru CLI smoke, 실제 CEF 통합과 네이티브 UI, 외부 사이트는 실행하지 않았다. ego SDK의 CDP는 대상 탭을 활성화하므로 이번 독립 검사로 Naru의 배경 선택 보존을 입증하지 않는다.

DOM 재검사 이후의 이동과 가림, mousedown 중 대상 변경, 서브프레임 교체와 사용자 입력 경합은 원자적으로 막지 못한다. 취소나 문서 변경으로 release를 생략하면 이전 페이지의 버튼 상태가 남을 수 있다. 실제 Naru의 배경 포인터, 팝업과 다운로드 권한, 파일 선택, 장시간 메모리와 반복 요청 회수는 남은 검증이다. source 변경을 최신 앱의 실행 완료로 설명하지 않는다. 앱 번들, release 바이너리와 서명을 만들거나 Naru, CEF 엔진과 HTTP 서버를 실행하지 않았고 키체인 승인 질문도 하지 않았다.

검증 공간 9는 finish를 정확히 한 번 호출해 closedSpace true, closedManagedLabels p1, preservedUnmanagedCount 0을 확인했다. 실제 응답은 `artifacts/pointer-native-v1-browser-cleanup.json`에 있다. 컴파일 세션 6096과 단위 세션 52497은 모두 exit 0으로 종료됐다. `target/.pointer-native-test-vfjpwg63`의 identity와 symlink 0개를 확인하고 파일 2,060개, 크기 합계 1,489,185,461바이트를 정리했다. 소스와 기존 앱, CLI 해시가 검사와 정리 전후 일치했다. `artifacts/pointer-native-v1-cleanup.json`이 근거이며 이 합계는 물리 디스크 회수량이나 RAM 측정값이 아니다.

최종 공백 검사와 변경 문서의 로컬 링크 103개 존재 검사를 통과했다. 소스, 검사 파일과 기존 앱, CLI의 해시 39개가 검증 기록과 일치했으며 임시 target 부재를 확인했다. `artifacts/pointer-native-v1-final-static.json`에 기록했다. HEAD는 `d3a0d8b`이고 이번 변경 15개 파일은 미커밋 상태다.

## 2026-10-16, Chromium Enter 변경의 커밋과 비공개 푸시 요청

사용자의 커밋 요청과 이어진 푸시 요청에 따라 CLI Enter 구현, 테스트, 문서의 14개 파일을 기존 `codex/navigation-readiness` 브랜치에 커밋하고 같은 원격 브랜치로 올리는 범위다. 새 클릭 입력 설계 조사는 파일 변경 없이 중단했다. 제품 코드를 추가로 수정하지 않았으며 아래 구현 기록의 미커밋 표시는 당시 상태다.

GitHub 조회에서 `FEABE/naru-browser`의 `isPrivate: true`와 기본 브랜치 main을 확인했다. 작업 전 로컬 HEAD와 원격 작업 브랜치는 모두 `0d7967d0fd2e4010f90da5478511ab6d660c43af`이었다. 제한 환경의 첫 네트워크 조회는 실패했으며 네트워크 접근을 허용한 조회는 성공했다. 이번 푸시 대상은 기본 브랜치가 아닌 기존 작업 브랜치다.

현재 소스와 Cargo 파일 31개, 기존 앱과 CLI 2개의 해시가 최종 Enter 컴파일과 Rust 단위 검사 기록에 일치했다. DOM bridge와 모의 검사 파일의 해시도 기존 94개 통과 결과와 일치했다. Rust 단위 324개, 기본 구성과 CLI 전용 구성 검사, 독립 Chromium 20개 검사 결과를 확인했으며 이번에는 같은 소스의 빌드와 동작 검사를 반복하지 않았다. `cargo fmt --all --check`와 `git diff --check`는 다시 통과했다.

최신 Naru CEF 통합, 네이티브 UI와 배경 선택 보존, 외부 사이트, 장시간 RAM 검증은 여전히 미완료다. 이번 작업에서 앱, 서버와 빌드를 실행하지 않았고 앞선 임시 target의 부재를 확인했다. 빌드 산출물, 사용자 프로필과 검증 원본은 Git 제외 상태를 유지한다. 커밋과 푸시의 실제 결과, 최종 로컬과 원격 SHA, 작업 트리는 `artifacts/commit-push-native-enter-20261016.json`에 기록한다.

## 2026-10-16, CLI Enter를 Chromium 입력으로 연결

직전 목표 턴은 Finder 위치 표시 변경의 `0d7967d` 커밋과 비공개 원격 SHA 확인까지 마친 진전이다. 이번에는 ego lite식 페이지 제어를 위해 Enter의 합성 이벤트와 수동 폼 제출을 Chromium 입력으로 바꿨다. 독립 브라우저 완성과 Preseller 내장 목표는 유지하며 전체 완료로 판정하지 않는다. 이번 새 변경은 커밋하거나 푸시하지 않았다.

`src/dom_bridge.js`의 press는 참조, 문서와 표시 상태를 확인한 뒤 포커스를 준비한다. 실제 activeElement를 열린 shadow host와 부모 iframe까지 최대 64단계로 확인하고 마지막 참조 검사를 수행한다. 포커스를 즉시 빼앗기거나 받을 수 없으면 FOCUS_NOT_REACHED로 거절한다. 준비 결과에는 `prepared:true`, `attempted:false`를 반환하고 KeyboardEvent와 form.requestSubmit을 실행하지 않는다. 준비 중 포커스 핸들러의 부작용이 없다는 뜻은 아니다.

`src/browser.rs`는 준비 결과를 확인한 뒤 같은 CEF 호스트에 Enter keyDown과 keyUp을 차례로 요청한다. keyDown에 CR 텍스트를 넣고 nativeVirtualKeyCode는 0으로 고정한다. OS 키 이벤트를 만드는 경로 대신 Chromium의 페이지 입력을 사용한다. 두 pending과 공유 배치 상태를 먼저 등록하며 두 엔진 접수 반환값, 두 응답과 enqueue 종료를 확인해야 한 번 성공한다. 준비에서 배치로 넘어갈 때도 탭의 진행 상태를 유지한다. 네이티브 getter 뒤와 각 제출 직전에 활성 요청, 작업 제어권, 문서 세대, renderer와 원래 Browser를 재검사한다. 첫 제출 중 문서나 제어권이 바뀌면 나머지 키를 보내지 않는다. 취소, 닫힘, renderer 종료와 연결 해제는 결과 불확실성을 남기며 늦은 응답으로 요청을 되살리지 않는다.

최종 성공은 `input:"chromium"`, `attempted:true`, `dispatchAcknowledged:true`로 두 엔진 응답만 표현한다. 예전 trusted와 formSubmissionRequested는 제거한다. 입력 결과는 다시 읽어 확인하며 자동 반복이나 합성 입력 fallback은 없다. 기존 Runtime.evaluate 준비 단계는 CEF의 비동기 결과 계약을 따르고, 새 두 입력 배치는 응답이 호출 중 도착하는 경우도 보수적으로 처리한다. 두 엔진 요청의 원자적 처리나 IPC 사이의 대상 고정은 보장하지 않는다.

`src/cli.rs`의 도움말을 Enter 지원 범위에 맞췄다. `docs/keyboard-input.md`에 동작, 응답 변경, 포커스와 문서 변경 경계, 한계를 기록하고 README, 아키텍처, 제품 계획과 메모리 문서를 갱신했다. 새로운 저장소, 상주 프로세스, 웹 UI renderer와 타이머는 없다. 고정 크기의 배치 상태와 원래 Browser 참조 하나를 요청 수명 동안 유지하며 기존 만료와 취소 경로로 해제한다. 실제 RAM 절감량을 측정한 결과는 아니다.

검증은 다음과 같이 구분한다.

- 모의 DOM은 최종 94개가 전부 통과했다. 준비 전용 계약과 즉시 포커스 이동, 무시된 focus, Shadow DOM과 iframe의 연결, 문서와 참조 변경, 예외 정제, 키와 제출 미발행을 다룬다. 변경 전 새 기대 조건으로 실행한 94개는 84개 통과와 10개 실패였다. 최종 bridge SHA-256은 `3d392703fb285e03d4a70c9d952158dbcce9957ddf23d68c8198cfec9fdbf17d`이며 원본과 해시는 `artifacts/keyboard-native-v2-dom-before.*`, `keyboard-native-v2-dom-after.*`에 있다.
- 최종 Rust 단위 324개가 전부 통과했다. 새 회귀 8개는 실제 reserve, enqueue, devtools_result와 TAB_BUSY, revoke, detach, renderer 경로를 stub 전송과 함께 실행한다. 동기 응답 뒤 반환값 0, 일부 응답과 중복 응답, 늦은 응답, getter 중 문서 변경, 첫 키 뒤 변경과 취소, 준비 성공 계약 불일치를 포함한다. CEF 엔진을 실행한 검사는 아니다. 오프라인 잠금 의존성 단위 실행 파일 컴파일, 기본 구성과 CLI 전용 구성의 전체 타깃 검사, format도 통과했다. 원본은 `artifacts/keyboard-native-v2-check.json`, `keyboard-native-v2-rust-tests.json`이다. 기존 test 전용 observe_history, page_find_result, bookmark_completed의 dead_code 경고 3개만 확인했다.
- 독립 ego lite Chromium 152의 자체 공간 8에서 nativeVirtualKeyCode 0의 기본 동작을 관찰했다. rawDown, char, up 방식과 keyDown 텍스트, keyUp 방식의 19개 관찰 및 SDK press 비교 1개를 기록했다. 버튼, textarea, 편집 영역, 기본 submitter, keydown 취소와 포커스 경합을 확인했다. 최초 스크립트 실행은 브라우저 명령 전에 await 구문 오류로 끝났으며 로그를 보존하고 실행 래퍼만 수정했다. 근거는 `artifacts/keyboard-native-v2-native0-probe.json`과 실행 로그다.
- 같은 공간에서 최종 bridge를 isolated world에 넣고 main world fixture와 연결한 20개 검사가 모두 통과했다. main, 중첩 Shadow DOM과 iframe, 서로 중첩된 shadow와 iframe, 버튼과 줄바꿈, 기본 submitter와 취소, 포커스 이동 거절, focus 중 대상 삭제와 미지원 키를 포함한다. keydown이 포커스를 옮기면 후속 입력이 새 대상으로 가는 한계도 관찰했다. `artifacts/keyboard-native-v2-bridge-probe.json`에 실제 값, 판단과 소스 해시를 기록했다. ego의 공개 CDP는 대상 탭을 자동 활성화하므로 Naru의 배경 선택 보존, 같은 UI 턴의 CEF 접수, 물리 키 입력 검증으로 확대하지 않는다.
- 설치된 CEF 헤더의 Chromium 버전 156.0.8078.17과 같은 태그의 공식 InputHandler, RenderWidgetHostImpl 소스를 확인했다. native code 0에서 브라우저 UI로의 미처리 키 전달을 막는 분기와 페이지 포커스 전달을 확인했다. URL, 해시와 관련 원문은 `artifacts/keyboard-native-chromium156-source.json`에 있다. 플랫폼 소스 확인을 실제 Naru 동작 증거로 대신하지 않는다.
- `tests/fixtures/native_press.html`, `tests/native_press_smoke.py`를 추가했다. 기존 `tests/smoke.py`, `tests/shadow_smoke.py`는 새 응답 계약과 실제 제출 결과를 확인하도록 바꿨다. iframe의 두 입력 폼에는 native implicit submit 대상 버튼을 추가했다. Python AST 3개와 inline JavaScript 구문 7개는 통과했으며 근거는 `artifacts/keyboard-native-v2-smoke-static.json`이다. 실제 Naru CLI와 CEF smoke는 실행하지 않았다.

최신 Naru의 실제 배경 Enter, 네이티브 선택과 입력기, 사용자 입력과의 경합, 외부 서비스와 장시간 RAM 검증은 남아 있다. 포커스 준비 직후나 keydown 처리 중의 대상 변경, 서브프레임 교체는 원자적으로 막지 못한다. 중간 변경 때문에 keyUp을 생략하면 이전 페이지의 키 상태가 남을 수 있으며 자동 재전송으로 복구하지 않는다. 지원 키는 Enter 한 개이고 Tab, 화살표, 조합키와 IME는 후속 범위다. 앱 번들, release 실행 파일과 서명을 만들거나 Naru, HTTP 서버를 실행하지 않았고 키체인 승인 질문도 하지 않았다.

검증 공간 8은 finish를 정확히 한 번 호출했다. 실제 응답의 closedSpace true, closedManagedLabels p1, preservedUnmanagedCount 0을 `artifacts/keyboard-native-v2-cleanup-space.json`에 보존했다. 기존 사용자 탭과 프로필에 정리 명령을 보내지 않았다. 컴파일 세션 42905와 단위 세션 80673은 모두 exit 0으로 종료됐다. 정확한 `target/.keyboard-native-test-lu8b_b5o`의 identity와 symlink 0개를 확인한 뒤 파일 2,060개, 크기 합계 1,486,752,774바이트를 제거했다. 소스와 기존 앱, CLI의 해시는 컴파일, 단위 검사와 정리 전후 같았다. `artifacts/keyboard-native-v2-cleanup.json`이 근거이며 파일 크기 합계를 물리 디스크 회수량으로 해석하지 않는다.

최종 공백 검사와 변경 문서의 로컬 링크 99개 존재 검사가 통과했다. 소스, 검사 파일과 기존 앱, CLI의 해시 38개가 검증 기록과 일치했고 임시 target의 부재를 확인했다. `artifacts/keyboard-native-v2-final-static.json`에 기록했다. HEAD는 `0d7967d`를 유지한다.

## 2026-10-16, Finder 위치 표시 변경의 커밋과 비공개 푸시 요청

사용자의 커밋 요청과 이어진 푸시 요청에 따라 기존 Finder 위치 표시 변경 11개 파일을 `codex/navigation-readiness`에 커밋하고 기존 `origin`의 같은 브랜치로 올리는 범위다. 제품 코드를 추가로 변경하지 않았다. 아래 구현 기록의 미커밋 표시는 해당 작업 종료 당시의 상태다.

GitHub 조회에서 `FEABE/naru-browser`의 `isPrivate: true`를 확인했다. 작업 전 로컬 HEAD와 원격 작업 브랜치는 모두 `d5b66c5148d86d57098a16245733b470c337f4fb`이었다. 제한 환경의 첫 네트워크 조회는 실패했으며 네트워크 접근을 허용한 조회는 성공했다. 기본 브랜치는 main이며 이번 대상은 기존 작업 브랜치다.

현재 소스와 Cargo 파일, 보존한 앱과 CLI의 SHA-256 33개가 최종 Finder v2 검증 기록과 모두 일치했다. Rust 단위 316개 통과와 오프라인 컴파일, 기본 구성과 CLI 전용 구성 검사의 기존 결과를 확인했다. 이번에는 같은 소스의 빌드와 단위 검사를 반복하지 않았고 `cargo fmt --all --check`, `git diff --check`를 다시 통과했다. 읽기 전용 변경 검토에서 추가 비밀값, 개인 데이터, 바이너리와 빌드 산출물 유입이나 확정 차단 결함을 발견하지 못했다.

최신 Naru의 다운로드부터 Finder 표시까지 이어지는 CEF 통합, 실제 Finder 선택과 네이티브 UI, 외부 사이트, 장시간 RAM 검증은 미완료다. 이번 커밋 작업에서는 Naru 앱, 서버와 새 빌드를 실행하지 않았다. 직전 검사 target의 부재와 기존 앱, CLI의 해시 보존을 확인했다. 빌드 산출물, 프로필과 검사 원본은 Git 제외 상태를 유지한다. 커밋과 푸시의 실제 결과, 로컬과 원격 SHA, 최종 작업 트리는 `artifacts/commit-push-finder-reveal-20261016.json`에 기록한다. 독립 브라우저 완성과 Preseller 내장 목표, 탭 복원 보류는 유지한다.

### 앞서 시작한 키 입력 조사와 정리

커밋 요청 전 진행하던 Enter 입력 조사는 제품 변경 없이 종료했다. 독립 ego lite Chromium 152의 자체 공간 7에서 20개 사례를 관찰했다. `keyDown`의 Enter 텍스트와 `keyUp` 두 메시지로 버튼 클릭, 줄바꿈, 기본 제출 버튼과 keydown 취소를 확인했다. 페이지 핸들러가 포커스를 옮기면 입력 대상도 바뀌었다. 공개 `Page.cdp` 구현은 Runtime과 Input 명령마다 대상 탭을 활성화하므로 배경 탭의 선택 보존은 검증하지 못했다. Naru CEF 통합이나 물리 키보드 검증 결과로 해석하지 않는다. 근거는 `artifacts/keyboard-native-v1-summary.json`과 연결된 원본이다.

해당 공간은 `finish({keep: []})`를 정확히 한 번 호출했다. 실제 응답의 `closedSpace: true`, `closedManagedLabels: ["p1", "p2"]`, `preservedUnmanagedCount: 0`을 `artifacts/keyboard-native-v1-cleanup.json`에 보존했다. 기존 사용자 탭과 프로필에 정리 명령을 보내지 않았으며 새 서버나 Naru 빌드는 없다.

## 2026-10-16, 완료 다운로드의 Finder 위치 표시

직전 작업은 `d5b66c5` 커밋과 비공개 원격 SHA 일치 확인까지 완료한 진전이다. 활성 목표를 이어 Aside식 일상 사용에서 다운로드 완료 파일을 찾는 흐름을 추가했다. 이번 새 변경은 커밋하거나 푸시하지 않았으며 독립 브라우저 전체와 Preseller 내장 목표는 계속 미완료다.

`src/downloads.rs`의 `update_with_approval`은 기존 Downloading 또는 Canceling 항목에서 이번 Complete 콜백의 유효한 경로가 사용자가 승인한 경로와 정확히 같을 때만 확인 여부를 설정한다. 이전 진행 경로가 일치해도 완료 경로가 누락되면 확인하지 않는다. terminal 불변 정책과 기존 경로를 재사용하므로 별도 경로 문자열을 더 보관하지 않는다. 기록을 지우거나 최대 24개 목록에서 제거하면 확인 정보도 함께 사라진다.

`src/download_runtime.rs`는 같은 borrow에서 승인값을 읽고 모델을 갱신한 뒤 기존대로 terminal 콜백을 해제한다. 표시 요청 때는 현재 완료 기록과 저장창, 종료 상태를 다시 확인하고 경로 하나만 복사한 뒤 borrow 밖에서 호출한다. 완료 파일은 출처 탭이나 AI 작업이 끝나도 찾을 수 있다. `src/mac.rs`는 `NSWorkspace::selectFile_inFileViewerRootedAtPath`로 위치 표시를 요청한다. `Cargo.toml`은 기존 AppKit 의존성의 NSWorkspace 기능만 활성화했다. 파일 실행과 사전 파일시스템 조회는 추가하지 않았다.

`src/control_panel.rs`는 행마다 Finder 버튼을 만들어 기존 저장 버튼 자리에서 재사용한다. 저장과 별도의 selector를 사용하며 접수 탭 보기 버튼을 유지한다. 네이티브 창, 패널과 IME를 조회한 뒤 현재 버튼과 표시 가능 여부를 확인한다. 반환 뒤에도 동일한 패널과 행일 때만 안내를 표시하며 탭, 공간, 주소 초안과 AI 제어권을 바꾸지 않는다. true 반환은 Finder 표시 요청의 결과이며 실제 화면 선택의 성공 증거로 설명하지 않는다. README, 아키텍처, 다운로드 설계, 제품 계획과 메모리 문서에 계약과 한계를 반영했다.

검증은 다음과 같이 구분한다.

- 최종 v2의 Rust 단위 316개가 전부 통과했다. 새 회귀 5개는 완료 경로의 누락과 불일치, 승인 없음과 잘못된 경로, 취소와 완료 경합, terminal 이후 변경 차단, 기록 삭제와 재사용 및 상한 제거, 콜백 해제 뒤 경로 유지, 저장창과 종료 경계를 다룬다. UI의 한 줄 조건을 그대로 반복하는 새 테스트는 제거하고 기존 좁은 폭 geometry 검사를 유지했다. 실제 화면 렌더링 검사가 아니다.
- 오프라인 잠금 의존성의 단위 실행 파일 컴파일, 기본 구성과 CLI 전용 구성의 전체 타깃 검사, format이 모두 통과했다. v1에서도 316개가 통과했으나 컴파일 경고에서 안전한 NSWorkspace 호출을 불필요하게 unsafe로 감싼 것을 확인해 제거했다. 최종 v2를 다시 컴파일하고 소켓 접근이 가능한 환경에서 방출된 동일 실행 파일만 검사했다. 두 실행을 합산하지 않는다. 최종 소스와 기존 앱, CLI 해시는 컴파일과 단위 실행 전후 같았다. `artifacts/finder-reveal-v2-check.json`, `finder-reveal-v2-rust-tests.json`이 근거이며 이전 v1 결과도 보존했다.
- 모델, runtime, macOS와 UI의 독립 읽기 리뷰에서 추가 확정 차단 결함을 발견하지 못했다. 로컬 AppKit 바인딩과 SDK의 Finder API 선언은 확인했지만 실제 호출이나 네이티브 선택을 검증한 결과는 아니다. 모의 DOM은 변경이 없어 이번에 재실행하지 않았다.
- 독립 Finder 검사 준비에서 CUA 앱 목록은 조회됐지만 Finder 상태 조회가 응답하지 않았다. 상위 에이전트가 해당 조회를 중단했고 도구 결과는 418.9초 뒤 aborted by user로 종료됐다. 별도의 사용자 취소나 입력 충돌이 확인됐다는 뜻은 아니다. native 명령, 컴파일, Finder 표시 요청과 생성 파일은 모두 0개이며 실행 중인 exec 세션도 없다는 담당 에이전트 보고를 확인했다. `artifacts/finder-reveal-v1-native-not-run.json`은 이 보고를 정리한 기록이며 실제 API 응답 영수증이 아니다. 같은 요청을 재시작하지 않았다.
- 최신 Naru의 다운로드 저장부터 완료 콜백과 Finder 버튼까지의 통합 동작, 실제 Finder 선택, IME와 좁은 창 배치, 외부 사이트와 장시간 RAM은 미검증이다. 제품 앱 번들과 서명을 새로 만들거나 Naru, CEF 엔진, HTTP 서버를 실행하지 않았고 키체인 승인 질문이나 사용자 프로필 접근도 하지 않았다. 파일 이동, 삭제, 같은 경로의 교체와 symlink 변경을 추적하지 않으며 네트워크 볼륨의 지연도 남은 검증이다.

v1 컴파일 세션 4407과 단위 세션 65339, v2 컴파일 세션 43964와 단위 세션 86605는 모두 exit 0으로 종료됐다. 소스와 기존 앱, CLI 해시를 확인하고 각 검사에서 만든 정확한 임시 target만 제거했다. v1은 파일 2,061개, 1,483,542,938바이트였고 v2는 파일 2,060개, 1,483,537,862바이트였다. 각 폴더 identity와 symlink 0개를 확인했으며 정리 뒤 소스와 기존 바이너리 해시는 동일했다. `artifacts/finder-reveal-v1-cleanup.json`, `finder-reveal-v2-cleanup.json`에 기록했다. 이 값은 파일 크기 합계이며 물리 디스크 회수량이나 RAM 절감량이 아니다. 별도 Finder 실행 파일과 fixture는 만들어지지 않아 정리할 대상이 없다.

최종 diff 공백 검사와 변경 문서의 로컬 링크 99개 존재 검사를 통과했다. 현재 소스와 기존 앱, CLI가 최종 v2 검증 해시와 같고 두 임시 target이 없는 것을 다시 확인했다. `artifacts/finder-reveal-v2-final-static.json`에 기록했다.

## 2026-10-16, 검색 탐색과 입력 검증 변경의 커밋, 비공개 푸시 요청

사용자가 커밋에 이어 푸시까지 요청했다. 이번 범위는 기존 미커밋 검색 탐색, 일반 입력값과 편집 영역 입력 검증, 배경 탭 실패 표시와 관련 문서, 검사 파일 19개를 `codex/navigation-readiness`에 커밋하고 기존 `origin`으로 푸시하는 작업이다. 새로운 제품 동작은 추가하지 않았다. 아래 두 작업 기록의 미커밋 표시는 각 구현 작업이 끝났을 당시 상태이며 이번 커밋 범위에 포함한다.

GitHub에서 `FEABE/naru-browser`의 `isPrivate: true`를 확인했다. 원격 작업 브랜치와 로컬 HEAD는 작업 전 모두 `a1e97c775c04fc4eb4bf139f964234593868a2e8`이었다. 최초 원격 조회는 제한 환경의 DNS 접근 오류로 실패했고 네트워크 접근을 허용한 조회는 성공했다.

현재 소스와 Cargo 파일, 기존 앱과 CLI, 모의 DOM 검사 파일의 SHA-256 34개가 직전 검증 기록과 모두 일치했다. 직전 Rust 단위 311개, 모의 DOM 88개 통과와 오프라인 컴파일, 기본 구성과 CLI 전용 구성 검사 결과를 확인했으며 이번에는 같은 검사를 재실행하지 않았다. `git diff --check`도 통과했다. 실제 Naru CEF 통합, 네이티브 검색 조작과 IME, 외부 사이트, 장시간 RAM 검증은 여전히 미실행이며 독립 Chromium 검사와 구분한다.

이번 작업에서는 앱과 서버를 실행하거나 새 빌드를 만들지 않았다. 직전 임시 검사 target의 부재를 다시 확인했으며 기존 앱과 CLI는 변경하지 않았다. 빌드 산출물, 로컬 프로필과 검사 로그는 Git 제외 상태를 유지한다. 커밋과 푸시 후 실제 로컬, 원격 SHA 및 작업 트리 상태는 `artifacts/commit-push-search-fill-20261016.json`에 별도로 기록한다. 독립 브라우저 완성과 Preseller 내장 목표, 탭 복원 보류는 유지한다.

## 2026-10-16, 통합 검색의 새 탐색과 일반 입력값 검증

직전 목표 작업은 배경 탭 실패 표시와 편집 영역 입력의 구현, 단위·독립 Chromium 검증으로 진전이 있었다. 이번에는 Aside식 통합 검색에서 새 탐색으로 이어지는 흐름과 ego lite식 CLI 입력의 성공 판정을 보완했다. 기존 미커밋 변경을 보존했으며 새 커밋, 푸시와 배포는 하지 않았다. 독립 브라우저 전체와 Preseller 내장 목표는 계속 미완료다.

`src/address.rs`는 기존 주소 해석을 `resolve_with_kind`로 공유하고 직접 입력한 주소와 생성한 Google 검색을 구별한다. 기존 resolve의 URL·오류 계약은 유지한다. `src/tab_search.rs`, `src/control_panel.rs`는 내부 결과 아래에 주소 열기 또는 Google 검색 행을 추가한다. 내부 결과를 우선하며 새 행을 포함해 40개, 입력 200자 상한을 유지한다. 초과 입력이 잘렸다면 같은 bounded 문자열 재통지나 모드 왕복으로 새 탐색을 다시 허용하지 않는다. 입력 중에는 외부 요청을 보내지 않는다.

명시적 행 실행은 검색 세대, 실제 필드 문자열과 IME, 모드·선택·표시 공간·창·오버레이를 재확인하고 `src/browser.rs::open_search_url → open_saved_user_url`의 기존 사용자 탭 재사용과 배경 준비 경로로 보낸다. 성공한 경우만 내 공간을 선택하고 검색을 닫는다. AI 작업의 상태와 revision은 바꾸지 않으며 동일 검색의 실패만 안내하고 사용자가 바꾼 새 검색을 덮어쓰지 않는다. 리뷰에서 History만 있던 정확한 URL의 Query를 클릭하면 새 탭에 의한 중복 제거가 선택을 바꿔 정상 생성까지 취소하는 P2를 찾았다. 네이티브 검증 뒤 클릭한 Query를 실행 선택으로 고정하고, 실제 combined_results와 reconcile을 사용하는 History→Tab→History 회귀를 추가했다. 이 결함은 호출 흐름으로 발견했으며 수정 전 앱 재현이나 red 테스트를 실행한 결과는 아니다.

`src/dom_bridge.js`의 일반 input과 textarea fill은 focus, native setter, input과 change 순서를 유지하면서 단계 사이 대상·프레임·문서·참조 세대·입력 종류와 편집 조건을 확인한다. 마지막 원문서 프로토타입 value getter가 요청 문자열과 정확히 일치할 때만 성공한다. 정규화와 페이지의 값 되돌림은 FILL_STATE_NOT_REACHED, 대상 변경은 FILL_STATE_UNCONFIRMED로 구분한다. 요청값과 실제 값, 페이지 예외는 오류에 넣지 않고 기존 Rust fill 오류의 attempted와 trusted boolean 두 개를 사용한다. CRLF 같은 정규화도 엄격한 원문 비교에서는 미도달이다. 새 beforeinput과 이벤트 없는 무동작, 자동 재시도는 추가하지 않았다.

검증은 다음과 같이 구분한다.

- 최종 Rust 단위 311개가 전부 통과했다. 이번 검색의 상태·UI 실행 경계 회귀 7개와 주소 분류 회귀 1개를 포함한다. 오프라인 잠금 의존성 컴파일, 기본 구성과 CLI 전용 구성의 전체 타깃 검사, format도 통과했다. v1 컴파일 뒤 독립 리뷰에서 위 P2를 찾아 같은 격리 target을 재사용해 v2를 다시 컴파일했다. v1 단위 실행은 하지 않았고, 최종 v2에서 방출된 실행 파일의 identity를 확인해 단위 검사만 소켓 허용 환경에서 실행했다. 이전 작업에서 확인한 제한 환경의 Unix socket 오류를 이번에 반복하지 않았다. `artifacts/search-fill-v1-check.json`, `search-fill-v2-check.json`, `search-fill-v2-rust-tests.json`이 근거다. CEF 엔진과 AppKit 입력을 실행한 검사는 아니다.
- 모의 DOM은 새 회귀 7개를 추가했다. 수정 전에는 새 회귀 1개 통과·6개 실패와 기존 81개 skip을 기록했고, 수정 후 전체 88개가 통과했다. 원본 로그와 소스 해시는 `artifacts/native-fill-v1-dom-before.*`, `native-fill-v1-dom-tests.*`에 있다. 값 정규화 모형, 핸들러 되돌림과 대상 변경, native getter 우회와 오류의 값 비노출을 검사했다.
- 독립 ego lite Chromium 152의 자체 공간 6에서 수정 전 23사례를 관찰해 값 미달이나 대상 제거 뒤에도 성공으로 반환하는 13사례를 재현했다. 최종 bridge SHA-256 `c35038156a25a24939444c06208cfc73f6540810aff8bad05921e5648f61af7e`는 별도 36개 검사를 모두 통과했다. main world의 fixture와 isolated world의 bridge, 페이지 단위 포커스 에뮬레이션을 사용했다. 숫자·날짜·문자열 정규화, input/change 되돌림, 대상·타입·상태 변경, own getter, Shadow DOM과 같은 출처 iframe, 문서 교체, 가상 비밀번호 오류를 확인했다. `artifacts/native-fill-v1-baseline-probe.json`, `native-fill-v1-final-probe.json`, `native-fill-v1-summary.json`이 근거다. Naru CEF나 물리 입력의 검증으로 확대하지 않는다.
- `tests/fixtures/native_fill.html`, `tests/native_fill_smoke.py`는 실제 Naru용 후속 검사를 위해 추가했으며 실행하지 않았다. AST와 inline JavaScript 구문 검사만 통과했다. 읽기 리뷰에서 Python dict 비교가 1/0도 boolean처럼 통과시키는 문제를 찾아 키 집합과 is 검사로 수정하고 AST를 다시 확인했다. `artifacts/native-fill-v1-fixture-syntax.json`, `native-fill-v1-smoke-static-v2.json`에 이전과 최종 해시를 구분했다. 검색 P2 수정의 독립 읽기 재검토에서는 추가 확정 결함을 발견하지 못했다.
- 최신 Naru의 검색 행 클릭·Enter·실제 탭 생성과 실패, IME·주소 초안·AI 선택 보존, 실제 CEF fill 전달, 외부 사이트와 장시간 RAM은 미검증이다. 앱 번들·release 실행 파일과 서명을 새로 만들거나 Naru를 실행하지 않았으며 키체인과 사용자 프로필에 접근하거나 승인 질문을 하지 않았다. 네이티브 windowed 탭에 windowless 전용 WasHidden API가 없다는 이유로 결함을 단정하거나 자동 폐기를 추가하지 않았다. 재시작 탭 복원과 확대의 보류도 유지한다.

README, 아키텍처와 제품 계획, 메모리 문서와 편집 영역 계약을 갱신하고 `docs/search-navigation.md`, `docs/native-fill-control.md`를 추가했다. 새 웹 UI renderer, 상주 worker, 타이머와 저장소는 없다. 일반 value 확인은 페이지의 실제 문자열을 임시로 읽으므로 큰 값의 읽기 비용까지 없애거나 전체 RAM을 절감했다고 설명하지 않는다.

ego 검사 공간은 포커스 에뮬레이션을 해제한 뒤 finish를 정확히 한 번 호출했다. 실제 응답의 closedSpace true, closedManagedLabels p1, preservedUnmanagedCount 0을 `artifacts/native-fill-v1-cleanup.json`에 보존했다. 새 HTTP 서버를 실행하지 않았고 기존 사용자 페이지는 조작하지 않았다. Cargo 세션 12586, 42924와 단위 세션 62866의 exit 0을 확인한 뒤 정확한 `target/.search-fill-test-j9qqwlki`만 제거했다. 파일 2,353개·크기 합계 1,647,449,390바이트·symlink 0개와 폴더 identity를 확인했으며 소스·Cargo와 기존 앱·CLI 해시는 정리 전후 같았다. `artifacts/search-fill-v2-cleanup.json`, `search-fill-v2-processes.json`이 근거다. 파일 크기 합계는 물리 디스크 회수량이나 RAM 감소량이 아니다.

최종 diff 공백 검사와 변경 문서의 로컬 링크 97개 존재 검사를 통과했다. 현재 src·Cargo와 기존 앱·CLI는 최종 Rust 검증 해시와 같고, DOM 소스와 모의 검사 파일도 실행 당시 해시와 같았다. 임시 target의 부재를 다시 확인했다. 근거는 `artifacts/search-fill-v2-final-static.json`이다.

## 2026-10-16, 편집 영역 입력과 배경 탭 실패 표시

직전 작업은 검증한 변경을 `a1e97c7`로 커밋하고 비공개 원격 SHA 일치를 확인한 진전이다. 활성 목표를 이어 Aside의 일상 사용성과 ego lite의 AI 폼 제어 공백 두 가지를 보완했다. 독립 브라우저 전체와 Preseller 내장은 여전히 미완료다. 이번 추가 변경은 별도 미커밋 상태이며 커밋, 푸시와 배포는 하지 않았다.

`src/control_panel.rs`는 기존 탭 행의 갱신 비교, 아이콘, 툴팁과 접근성 문구에 `ShellTab.failure`를 연결한다. 같은 공간의 배경 탭이 실패해도 표시하고, 실패 아이콘을 우선하되 AI 작업과 로딩, 음소거 설명을 함께 유지한다. 복구 중 오래된 renderer 종료 기록을 계속 표시하지 않도록 현재 탐색 상태가 투영한 failure만 사용한다. 기존 컨트롤을 재사용하며 선택, 제어권과 재시도 동작은 바꾸지 않는다. 실패만 바뀐 행의 갱신과 복구, 표시 우선순위, 복합 상태 문구 회귀 3개를 추가했다.

`src/dom_bridge.js`는 기존 fill을 명시적 HTML contenteditable 영역의 전체 텍스트 교체로 확장한다. 빈 속성과 plaintext-only도 관찰한다. 단순 텍스트가 이미 같은 경우를 제외하고 focus, 가용성 재검사, 취소 가능한 beforeinput, 텍스트 교체, input 순서로 실행한다. 각 단계의 참조와 문서, 편집 조건을 확인하고 페이지가 교체 결과를 바꾸면 미확정과 불일치를 구분한다. HTML 문자열은 문자 그대로 넣으며 서식과 임의 편집기 모델, Selection과 undo를 보존하는 기능은 아니다. Naru의 부모 비편집 조건은 지원 정책이며 HTML 표준의 editing host 정의로 단정하지 않는다.

`src/browser.rs`는 원래 fill 요청과 반환 action이 일치할 때만 attempted와 trusted boolean 두 개를 오류 상세에 전달한다. 기존 check의 네 boolean 제한과 취소 후 outcomeUnknown, 늦은 응답 차단을 유지한다. 오류 상세와 취소 회귀를 두 요청으로 확장했다. `src/cli.rs` 도움말, README, 아키텍처와 제품 계획, 메모리 문서, 새 `docs/contenteditable-control.md`에 계약을 반영했다. 아키텍처 표에서 이미 구현된 본문 입력 인계를 미구현으로 적던 항목도 실제 mac 이벤트 호출과 본문의 기존 설명에 맞췄다.

검증 결과는 다음과 같이 구분한다.

- Rust의 오프라인 잠금 의존성 단위 실행 파일 컴파일, 기본 구성과 CLI 전용 구성의 전체 타깃 검사, format이 통과했다. 최초 단위 실행은 300개 통과와 기존 Unix socket bind 권한 오류 3개 실패였다. 원본을 보존하고 같은 emitted 바이너리만 소켓 접근이 가능한 환경에서 실행해 303개 전부 통과했다. 컴파일과 실행 전후 소스와 기존 앱, CLI 해시가 같았다. `artifacts/editing-status-v1-check.json`, `editing-status-v1-rust-tests.json`, `editing-status-v1-rust-tests-escalated.json`이 근거다. 실제 CEF 엔진이나 AppKit 입력을 실행한 검사는 아니다.
- 모의 DOM은 기존 70개와 새 11개, 총 81개가 통과했다. 최초 성공은 도구 출력만 남아 동일 소스로 한 번 재실행해 원본 로그와 해시를 보존했다. `artifacts/contenteditable-v1-dom-tests.log`, 같은 이름의 JSON에 근거를 남겼다. 전체 textContent 조회 없이 기존 읽기 예산과 4,096 단위 이하의 텍스트 비교를 사용하고, 입력 취소와 단계별 대상 변경, 불일치, iframe 문맥을 검사했다.
- ego lite의 자체 공간 5에 격리된 JavaScript 환경을 만들고 현재 bridge SHA-256 `f9a9e98cfcb13929a786184d3f69ddeb2941c023bbaf8d2434a6f3b98faa66c9`를 주입했다. Chromium 152의 본 검사 26개 중 25개가 통과했고, 중첩 요소에 ref가 있을 것이라는 검사 가정 한 개가 실패했다. 원본을 보존하고 무tabindex 중첩 요소의 관찰 제외와 tabindex가 있는 ref의 fill 거절을 나눠 검사해 후속 2개가 통과했다. 제품 코드를 검사 가정에 맞춰 바꾸지 않았다. 일반 입력과 textarea 회귀, 열린 Shadow DOM과 같은 출처 iframe, focus와 beforeinput 중 문서 교체도 확인했다. iframe 교체는 동기 document.open/write/close fixture이며 원격 탐색 경합 검증은 아니다. 본 검사에는 페이지 단위 포커스 에뮬레이션을 사용했다. `artifacts/contenteditable-summary.json`, `contenteditable-final-probe.json`, `contenteditable-nested-probe.json`이 근거다. 최신 Naru CEF 연결이나 물리 입력 증거로 확대하지 않는다.
- `tests/fixtures/contenteditable.html`, `tests/contenteditable_smoke.py`는 후속 로컬 CEF 검사를 위해 추가했다. 기존 CLI와 HTTP fixture 도우미를 재사용하고 자체 작업과 탭의 정리, 사용자 선택 보존을 검사하도록 작성했다. Python AST와 inline JavaScript 구문 검사는 통과했지만 실제 CLI smoke는 실행하지 않았다. 근거는 `artifacts/contenteditable-v1-fixture-syntax.json`이다. 독립 읽기 리뷰에서 DOM 계약, Rust 오류 전달, fixture와 정리에 추가 확정 차단 결함은 발견하지 못했다.
- 최신 Naru의 배경 실패 표시와 접근성, 실제 CEF fill 전달, 외부 편집기, 물리적 IME 입력과 장시간 RAM은 미검증이다. 앱 번들, release 바이너리와 서명을 새로 만들거나 Naru를 실행하지 않았으며 키체인이나 사용자 프로필에 접근하지 않았다. 기존 input과 textarea의 합성 값 설정 경로는 유지한다. 탭 복원과 페이지 확대 보류도 유지한다.

독립 Chromium 검사는 포커스 에뮬레이션을 해제하고 공간 5에 finish를 정확히 한 번 호출해 정리했다. `artifacts/contenteditable-cleanup.json`에 기록했다. 최초 공간 생성 뒤 상대 경로로 로그를 쓰던 단계만 실패했고, 생성된 공간을 확인해 그대로 재사용했다. 새 로컬 서버는 만들지 않았고 기존 사용자 페이지를 조작하지 않았다.

검사 프로세스와 세션의 종료를 확인하고 격리 `target/.editing-status-test-q414jzec`만 제거했다. 삭제 전 identity와 파일 2,060개, 합계 1,476,949,211바이트, symlink 0개를 확인했다. 제한 환경에서 ps 조회가 차단됐을 때는 삭제하지 않았고, 지정 검사 PID 세 개의 부재를 별도 조회한 뒤 정리했다. 정리 전후 소스와 Cargo 파일, 기존 앱과 CLI 해시는 같고 해당 임시 폴더의 부재를 확인했다. 근거는 `artifacts/editing-status-v1-cleanup.json`, `editing-status-v1-processes.json`이다. 이 파일 크기 합계는 물리 디스크 회수량이나 RAM 절감량이 아니다. 기존 사용자 프로필과 탭은 보존했다.

최종 `git diff --check`와 변경 문서의 로컬 링크 85개 존재 검사를 통과했다. 현재 소스와 기존 앱, CLI 해시가 검증 기록과 일치하고 임시 target이 없는 것을 재확인했다. 근거는 `artifacts/editing-status-v1-final-static.json`이다.

## 2026-10-16, 검색 키와 요청 큐 보호 검증, 커밋과 비공개 푸시 요청

사용자가 커밋에 이어 푸시까지 요청했다. 기존 미커밋 자동 스크롤과 CLI check, 이번 검색 키 처리와 요청 큐 보호를 관련 문서와 함께 커밋하는 범위로 고정했다. GitHub에서 `FEABE/naru-browser`의 `isPrivate: true`를 확인했고, `origin`의 `codex/navigation-readiness`는 작업 전 로컬 HEAD와 같은 `b512838a1397a0ef4519a130300ab0b2ae294665`였다. 최초 GitHub 조회는 네트워크 제한으로 실패했고 네트워크 접근을 허용한 조회는 성공했다. 기본 브랜치나 배포 설정은 변경하지 않는다. 앞선 기록의 미커밋 표시는 당시 상태이며 이번 커밋 범위에 포함한다.

`src/control_panel.rs`는 검색 입력의 편집기가 현재 first responder일 때만 Enter와 위아래 화살표를 처리한다. 삭제 확인 중에는 결과 열기와 선택 이동을 막고 Enter 안내를 숨긴다. 입력 필드의 submit 경로에도 확인 상태 검사를 넣었다. 버튼에 포커스가 있을 때는 AppKit에 키를 전달하며 명시적인 결과 클릭과 Escape 동작은 유지한다. 순수 키 정책과 submit 상태 회귀 3개를 추가했다.

`src/browser.rs`는 UI에 게시한 요청 payload를 서버별 16개로 제한한다. 연결 worker가 시간 초과돼도 미실행 task는 자리를 유지하며 실행 종료나 마지막 task 참조 해제 때 반환한다. task 복제는 payload를 공유하고 실행 시 한 번만 꺼낸다. 새 요청을 게시하기 전 상한에 도달하면 `APP_BUSY`, `outcomeUnknown:false`를 반환한다. 새 worker나 타이머, 저장소는 추가하지 않았다. 큐 정체와 재연결, 중복 실행, 게시 실패와 폐기 모형, 서버별 독립 상한 회귀 3개를 추가했다. `docs/architecture.md`, `docs/memory.md`, `docs/history-design.md`, `docs/product-plan.md`에 계약과 한계를 반영했다.

검증 결과는 다음과 같이 구분한다.

- 오프라인 잠금 의존성의 단위 실행 파일 컴파일, 기본 구성과 CLI 전용 구성의 전체 타깃 검사, format이 통과했다. 최초 제한 환경 단위 검사는 297개 통과와 Unix socket bind 권한 오류 3개 실패였다. 원본 로그를 보존하고 같은 실행 파일만 소켓 접근이 가능한 환경에서 다시 실행해 300개 전부 통과했다. 실행 파일 SHA-256은 `a45f305ee886b9c8cd7163692c1c17290d29f549ee3e866ae9529ec0a9e66ee2`다. 소스와 기존 앱, CLI의 해시는 검사 전후 동일했다. 근거는 `artifacts/interaction-bounds-v1-check.json`, `interaction-bounds-v1-rust-tests.json`, `interaction-bounds-v1-rust-tests-escalated.json`과 해당 로그다.
- 큐 검사는 실제 Rust CEF task 래퍼의 clone, execute, drop을 사용하지만 엔진을 초기화하거나 실제 CEF UI를 정지시키지 않았다. native post 실패의 참조 해제는 바인딩과 소유권 계약을 검토한 범위이며 앱 실행으로 확인하지 않았다. 검색 키 검사는 순수 정책 검사로 실제 AppKit 포커스와 입력기 검증을 대신하지 않는다.
- 앞선 check 검증의 CLI, protocol, DOM bridge 소스 해시가 현재와 같음을 확인했다. 기존 모의 DOM 70개 통과 로그를 확인했으며 재실행하지 않았다. `tests/check_smoke.py` 구문 검사와 변경 문서의 로컬 링크 83개 존재 검사는 통과했다. 독립 검토에서 커밋 후보 16개 파일에 프로필, 비밀값, 바이너리와 빌드 산출물이 섞인 것은 발견하지 못했다.
- 최신 Naru의 로컬 CEF 통합, 네이티브 키 입력과 드래그, 외부 사이트, 장시간 사용과 RAM 측정은 미실행이다. 앱 실행과 재서명, 키체인 접근이나 추가 승인 요청은 하지 않았다. 독립 브라우저 전체 완성과 Preseller 내장은 남아 있고 재시작 탭 복원 보류도 유지한다.

격리 `target/.interaction-bounds-test-4jhzbtyf`의 identity를 확인한 뒤 파일 2,060개, 디렉터리 290개, 파일 크기 합계 1,476,453,544바이트, symlink 0개를 기록하고 해당 폴더만 제거했다. 모든 검사 세션의 종료와 폴더 부재를 확인했다. 소스와 기존 앱, CLI 해시는 정리 후에도 같으며 근거는 `artifacts/interaction-bounds-v1-cleanup.json`이다. 이 크기는 물리 디스크 회수량이나 RAM 절감량이 아니다. 새 제품 앱이나 서버는 실행하지 않았고 기존 프로필과 탭은 보존했다.

커밋과 푸시의 실제 명령 결과, 최종 로컬과 원격 SHA는 `artifacts/commit-push-20261016.json`에 별도로 기록한다. 검증 자료와 사용자 데이터, 앱 번들은 Git에 포함하지 않는다.

## 2026-10-15, CLI 체크박스와 라디오 상태 지정

직전 목표 작업은 탭 자동 스크롤의 구현과 검증으로 진전이 있었고, 이어진 사용자 푸시 요청은 원격 `b512838` 일치를 재확인해 마쳤다. 활성 목표를 재개한 이번 작업은 독립 Naru의 AI 폼 제어를 보완한다. 전체 브라우저 완성과 Preseller 내장은 미완료이며 목표를 작은 기능의 완료로 대체하지 않는다. 새 커밋, 푸시와 배포는 하지 않았다.

`src/cli.rs`, `src/protocol.rs`, `src/browser.rs`에 `check TAB REF true|false`와 작업 범위 명령을 연결했다. 정확한 boolean을 JSON 데이터로 전달하고 기존 작업 소유권, revision, 탐색 epoch와 mutation 취소 경로를 사용한다. 실제 요청이 check였을 때만 DOM 실패의 시도 여부와 현재 상태를 최대 네 boolean으로 오류 상세에 보존한다. 동작을 시도한 뒤 제어권이 바뀐 경우는 기존 `outcomeUnknown`을 유지하며 늦은 응답을 다시 전달하지 않는다.

`src/dom_bridge.js`는 현재 상태가 같으면 포커스와 클릭 없이 반환한다. 변경이 필요하면 focus 후 가용성을 다시 검사하고 DOM 클릭 한 번을 사용한다. 클릭 취소와 핸들러의 되돌림, 대상 제거와 타입 변경을 구분하며 성공으로 간주하거나 자동 재시도하지 않는다. 체크박스의 혼합 상태를 read에 표시하고 혼합 상태의 변경, 라디오 해제와 사용자 정의 ARIA 컨트롤은 거절한다. 동작 도중 새 read가 실행된 경우도 captured ref의 세대를 검사하도록 공통 참조 검사를 보완했다. source의 자동 스크롤 설명 주석 한 곳도 현재 동작에 맞췄다.

`tests/dom_bridge.test.mjs`에 네이티브 activation의 제한된 모형과 15개 회귀 검사를 추가했다. `tests/fixtures/check.html`, `tests/check_smoke.py`는 Naru의 후속 로컬 CEF 통합 검사용이다. `docs/check-control.md`에 CLI 응답과 한계를 기록하고 README, 아키텍처, 제품 계획, 메모리 문서를 갱신했다. 기존 사용자 입력 handoff와 접수된 다운로드 권한의 상태 갱신은 소스에서 이미 연결된 것을 확인했다. 제품 계획의 미완료 권한 항목을 숨긴 탭에서 CEF 전달 전 대기 감지로 구체화했으며 새 알림 기능을 구현한 것으로 표시하지 않는다.

검증 결과는 다음과 같이 구분한다.

- Rust는 offline, locked 단위 실행 파일 컴파일, 기본 구성과 CLI 전용 구성의 전체 타깃 검사, format을 통과했다. 최초 제한 환경의 단위 실행은 291개 통과와 Unix socket bind 권한 오류 3개 실패였다. 원본 로그를 보존하고 같은 emitted 바이너리만 로컬 소켓 허용 환경에서 재실행해 최종 294개 전부 통과했다. 이번 신규 8개는 CLI와 원본 JSON boolean, task allowlist와 소유권, epoch, 안전한 JSON 직렬화, 실패 상세의 필드 제한과 취소 후 응답을 검사한다. 실제 CEF 엔진 실행은 아니다. 근거는 `artifacts/check-control-v1-check.json`, `check-control-v1-rust-tests.json`, `check-control-v1-rust-tests-escalated.json`과 해당 로그다. 기존 단위 dead_code 3개, CLI 라이브러리 9개와 단위 get 경고 1개는 남는다.
- 모의 DOM 최종 v3는 70개 전부 통과했다. 최초 실행의 68개 통과와 2개 실패에서 focus나 클릭 핸들러의 재진입 read 뒤 참조 세대 검사 누락을 발견해 수정했다. 최초 전체 TAP 파일은 저장하지 않아 `artifacts/check-dom-v1-summary.json`에 실제 assertion을 전사한 요약임을 명시했다. 수정 뒤 v2의 70개 통과 로그를 보존했고, 이후 실제 Chromium에서 확인한 분리 요소의 후속 이벤트 생략을 모형과 검사에 반영한 최종 근거는 `artifacts/check-dom-v3.log`다. 제품 코드를 검사 모형에 맞춰 이벤트를 추가하지 않았다.
- 독립 Chromium은 설치된 ego lite 공간 4의 자체 로컬 HTTP 페이지에서 검사했다. 현재 bridge SHA-256 `0dc58288f0be294f1f8522d91d4cae496d7aa0f1ec678230caed99fe85bed4aa`를 격리된 JavaScript 환경에 주입해 24개 시나리오가 통과했다. 최초 실행은 14개 통과 뒤 분리된 input에서도 input/change가 발생할 것이라는 검사 가정에서 실패했다. 같은 페이지의 실제 상태와 HTML 규격을 확인해 기대값을 고쳤으며 실패 JSON을 보존했다. 이때 document.hasFocus는 false였고 focus 이벤트도 발생하지 않았다. 최종 실행은 검사 페이지에만 CDP 포커스 에뮬레이션을 사용해 focus 핸들러와 교체 보호까지 확인했다. 클릭은 untrusted였고 브라우저가 만든 후속 input/change는 별도로 기록했다. `artifacts/check-control-v1-ego-result.json`, `check-control-v1-ego-first-failure.json`이 근거다. 이는 Naru 소켓과 Rust 요청 전달, 실제 CEF 통합이나 macOS 포커스의 증거가 아니다.
- `check_smoke.py`와 fixture의 inline JavaScript는 구문 검사를 통과했다. 실제 Naru CEF smoke, 네이티브 UI, 외부 사이트, 장시간 실행과 RAM 측정은 미실행이다. 앱 서명과 번들을 새로 만들거나 Naru를 실행하지 않았다. 사용자 프로필과 키체인에 접근하거나 추가 승인을 요청하지 않았다. 독립 읽기 교차 리뷰에서는 추가 확정 blocker를 발견하지 못했다.

최종 `git diff --check`와 변경 문서의 로컬 링크 83개 존재 검사를 통과했다. 빌드와 단위 검사 세션은 끝났다. 동일 source와 retained 앱, CLI 해시를 확인하고 격리 `target/.check-control-test-b6ak2wp6`의 파일 2,060개, 크기 합계 1,475,279,110바이트, symlink 0개를 검사한 뒤 해당 폴더만 제거했다. 제거 후 source와 기존 실행 파일의 해시는 그대로였다. 이 합계는 물리 디스크 회수량이나 RAM 절감량이 아니다. `artifacts/check-control-v1-cleanup.json`에 근거를 남겼다.

ego 검사 공간은 포커스 에뮬레이션을 해제한 뒤 `finish({keep:[]})` 한 번으로 종료했다. 도구 응답에서 검사 페이지 p1과 공간 닫힘, 보존된 unmanaged 탭 0개를 확인했다. 기존 사용자 페이지는 조작하지 않았다. 임시 HTTP 서버 PID 31353, 실행 세션 80172는 Ctrl-C의 KeyboardInterrupt로 exit 1 종료를 확인했다. 최초 로컬 bind 제한과 ego 앱 bootstrap 제한, CLI 내부 상대 경로 파일 조회 실패는 각각 재시도 전에 상태를 확인했으며 같은 공간을 재사용했다. 제품 앱 프로세스는 실행하지 않았고 기존 Naru 프로세스 목록은 새로 조회하지 않았다. 기존 앱과 CLI, 프로필을 보존하고 검사 소스와 로그만 Git에서 제외한 artifacts에 남겼다.

## 2026-10-15, 현재 커밋의 비공개 원격 푸시 재확인

사용자의 “푸시까지 해야지” 요청에 따라 `codex/navigation-readiness`의 현재 커밋 `b512838a1397a0ef4519a130300ab0b2ae294665`를 기존 `origin`으로 푸시했다. GitHub 조회에서 `FEABE/naru-browser`의 `isPrivate: true`와 원격 커밋 일치를 확인했다. 최초 조회는 네트워크 제한으로 실패했으며 권한을 조정한 조회와 `git push --porcelain origin HEAD:refs/heads/codex/navigation-readiness`는 성공했다. 푸시 결과는 `[up to date]`였다.

추가 개발 중이던 편집을 중지하고 기존 미커밋 파일을 보존했다. 자동 스크롤 변경과 작성 중인 `check`의 Rust 연결, DOM 테스트, fixture는 이번 푸시에 포함하지 않았다. `check`의 DOM 구현과 통합 검증은 아직 미완료이며 성공한 기능으로 기록하지 않는다. 새 커밋과 배포는 하지 않았다. 관련 파일은 이 작업 기록이며 이번 푸시 확인 때문에 제품 동작을 변경하지 않았다.

이번 검증은 Git 상태, 원격 커밋, 저장소 공개 범위와 푸시 결과 확인으로 한정한다. 단위, 모의 DOM, 로컬 CEF 통합, 네이티브 UI와 외부 사이트 검사는 실행하지 않았다. 앱과 서버를 시작하거나 빌드 생성물을 만들지 않아 이번 요청으로 새로 정리할 프로세스와 생성물은 없다. 사용자 프로필과 기존 앱은 변경하지 않았다.

## 2026-10-15, 긴 탭 목록의 드래그 자동 스크롤

직전 커밋과 비공개 푸시는 `b512838a1397a0ef4519a130300ab0b2ae294665` 원격 일치까지 완료했다. 이후 활성 목표의 일상 사용성을 이어서 긴 탭 목록을 드래그로 정리할 수 있게 했다. 이번 추가 변경은 미커밋이며 추가 푸시와 배포는 하지 않았다. 이전 목표 작업은 코드와 검증 근거를 만든 진행으로 분류하며 전체 독립 브라우저 완성과 Preseller 내장은 여전히 미완료다.

`src/control_panel.rs`에서 탭 목록의 위아래 24pt 안에 포인터가 있으면 12pt씩 자동 스크롤한다. 작은 표시 영역에서는 가장자리 폭을 높이의 절반 이하로 줄인다. 네이티브 타이머 한 개를 명목 30Hz와 10% tolerance로 common mode와 event tracking mode에 등록한다. 포인터를 멈춰도 이동하고 중앙이나 문서 끝에서는 타이머만 해제해 드롭을 유지한다. 다시 가장자리로 이동하면 재시작한다. 드롭과 취소, 창 상태 변화와 종료는 기존 취소 경로에서 타이머를 해제한다. 대기 중 상주 타이머나 새 worker, 웹 renderer, 패키지와 저장소는 추가하지 않았다.

드래그 동안 저장한 window 좌표를 현재 문서 좌표로 변환한다. NSClipView의 제한된 세로 이동과 실제 clip bounds, viewport가 일치할 때만 예상 표시 영역을 갱신한다. 창과 문서, 원본 행과 clip frame, 가로 위치와 크기, 공간과 선택, 전체 순서가 바뀌면 취소한다. native 호출 사이에 RefCell borrow를 유지하지 않으며 세션과 타이머 identity를 다시 확인한다. 이전 타이머의 늦은 호출은 새 타이머를 건드리지 않는다. Ended 상태의 tick은 최종 드롭을 취소하지 않고 타이머만 멈춘다. 최신 viewport로 삽입 위치를 계산하고 기존 순서 변경 API로 드롭 시 한 번 반영한다. 선택과 AI 제어권은 직접 변경하지 않는다. README와 아키텍처, 제품 계획, 메모리, 탭 순서 문서에 동작과 검증 경계를 반영했다.

검증 결과는 다음과 같이 구분한다.

- 최종 `tab-autoscroll-v1`에서 오프라인 잠금 의존성 단위 실행 파일 빌드, 기본 구성과 CLI 전용 구성의 전체 타깃 컴파일, format이 모두 통과했다. Rust 단위 286개가 전부 통과했다. 신규 4개는 가장자리와 범위 검사, 고정 포인터의 끝 도달과 반대 방향 재시작, 허용한 세로 이동만 채택하는 조건, 기타 native 배치 변경 거절을 확인한다. 실제 NSTimer나 CEF 엔진을 실행하는 단위 검사는 아니다. 기존 Unix 소켓 검사 때문에 단위 실행 파일만 별도 권한으로 실행했다. 전체 src와 Cargo 파일 해시는 검사 전후 동일하다. 기존 단위 dead_code 3개, CLI 라이브러리 9개와 CLI 단위 get 경고 1개는 남는다. 근거는 `artifacts/tab-autoscroll-v1-check.json`과 구성별 로그다.
- 독립 소스 교차 리뷰에서 타이머와 세션 수명, 재진입, 늦은 호출, Ended 드롭 보존과 기존 취소 연결을 확인했고 추가 확정 P1, P2 결함은 발견하지 못했다. `git diff --check`와 변경 문서의 로컬 링크 검사도 통과했다. 기존 Graphify 그래프는 경로 탐색에만 사용했으며 새 드래그 코드를 포함하지 않아 현재 소스 호출 흐름으로 다시 확인했다.
- 분리된 숨긴 AppKit 창의 최초 제한 환경 실행은 macOS XPC 연결 오류로 10초 후 해당 PID 19207만 종료했다. 이후 GUI 실험 권한으로 실행한 PID 19339의 v1은 17개 검사와 21회 tick을 통과했다. 이 버전은 8pt 이동과 1e-5 오차 허용 비교를 사용했다. 제품과 스크롤바 설정 차이를 확인해 v2를 별도로 실행했고 최초 실패와 v1 원문 기록은 보존했다.
- v2는 제품과 같은 스크롤바 자동 숨김, contentSize에 맞춘 문서 너비, 12pt 이동, 30Hz와 10% tolerance를 사용했다. PID 21518, exit 0에서 17개 검사와 21회 tick을 통과했다. 기본 mode와 tracking mode에서 고정 window 포인터의 문서 좌표, clip bounds와 visibleRect 이동량을 정확한 동등 비교로 확인했다. 타이머 해제 뒤 150ms 동안 추가 호출과 스크롤은 0회였고 타이머와 대상 객체 해제를 확인했다. 자체 recognizer 메서드를 직접 호출한 플랫폼 API 검사이며 실제 Naru의 가장자리 정책 연결, 취소 이벤트나 물리적 드래그를 검증한 결과는 아니다. `artifacts/tab-autoscroll-native-probe/summary-cleanup-v2.json`, `run-v2.log`가 근거다.
- DOM bridge를 변경하지 않아 모의 DOM 검사는 반복하지 않았다. 최신 Naru 앱의 로컬 CEF 통합, 실제 드래그와 Escape, 입력기와 주소 초안, 배경 CLI 경합, 접근성과 외부 사이트, 장시간 사용과 RAM 측정은 미실행이다. 제품 앱 번들과 서명을 만들거나 CEF 엔진을 초기화하지 않았고 사용자 프로필과 키체인에 접근하거나 승인을 요청하지 않았다. 공간 간 드래그는 추가하지 않았으며 재시작 탭 복원 보류도 유지한다.

빌드와 단위 검사 세션은 종료됐다. 격리 `target/.tab-autoscroll-test-6zf33ody`의 파일 2,060개, 합계 1,473,321,507바이트와 symlink 0개를 확인한 뒤 해당 폴더만 제거했다. 기존 앱과 CLI, 전체 src와 Cargo 파일 해시는 정리 전후 동일하다. 독립 AppKit 검사 PID 19207, 19339, 21518의 부재를 확인했고 실행 파일 79,960바이트와 96,744바이트를 제거했다. 실행 소스와 로그는 Git에서 제외한 artifacts에 보존했다. 제품 앱과 서버는 시작하지 않았고 기존 Naru 프로세스는 다시 조회하지 않았다. 파일 크기 합계를 물리 디스크 회수량이나 RAM 절감량으로 설명하지 않는다.

## 2026-10-15, 탭 드래그 변경 커밋과 비공개 푸시 요청

사용자가 커밋에 이어 푸시까지 요청했다. 이번 범위는 기존 미커밋 탭 드래그 정렬과 관련 문서 10개를 `codex/navigation-readiness` 브랜치에 커밋하고 기존 `origin`으로 푸시하는 작업이다. 새로운 기능 구현은 하지 않았다. GitHub 조회에서 `FEABE/naru-browser`의 `isPrivate: true`를 확인했고 작업 전 원격 브랜치는 로컬 HEAD와 같은 `f0d60b1435d599180b30fea551c3846e46b3bd71`이었다. 최초 네트워크 제한으로 실패한 조회는 권한을 조정해 다시 확인했다.

현재 src 파일 29개의 해시가 `artifacts/tab-drag-v2-check.json`에 기록된 검증 소스와 모두 일치한다. 해당 기록의 Rust 단위 282개 통과, 기본 구성과 CLI 전용 구성의 전체 타깃 컴파일, format 통과 결과를 확인했다. 이번에는 빌드와 단위 검사를 반복하지 않았다. `git diff --check`와 커밋 후보 파일의 공통 비밀값 패턴 검사는 통과했다. 독립 읽기 검토에서도 변경 범위와 검증 기록이 일치했다.

실제 Naru의 로컬 CEF 통합, 네이티브 입력과 외부 사이트, RAM 측정은 새로 실행하지 않았다. 분리 AppKit 실험의 Escape 미검증과 가장자리 자동 스크롤 미구현은 유지하며 재시작 탭 복원도 보류 상태다. 앱과 서버를 실행하거나 새 빌드 산출물을 만들지 않았다. 이전 격리 테스트 target의 부재를 확인했으며 사용자 프로필과 기존 앱, CLI는 변경하지 않았다. 직전 기록의 미커밋과 미푸시는 당시 상태이며 이번 요청 범위에 포함한다.

## 2026-10-15, 사이드바에서 같은 공간의 탭 드래그 정렬

직전 요청의 커밋과 비공개 푸시는 `f0d60b1` 원격 일치까지 완료했다. 이후 활성 목표의 일상 브라우저 사용성을 진행해 기존 메뉴 정렬에 사이드바 드래그를 추가했다. 이번 변경은 별도 미커밋 상태이며 추가 푸시와 배포는 하지 않았다. 같은 공간의 표시 순서만 변경하며 공간 간 이동, 탭 복원이나 새 저장소는 추가하지 않는다.

`src/control_panel.rs`는 기존 선택 버튼에 `NSGestureRecognizer` 하위 클래스를 붙인다. 마우스 시작점에서 6pt 이상 움직이면 드래그로 전환한다. 작은 이동은 인식 실패 뒤 원래 이벤트를 NSButton으로 전달하며 별도 클릭을 합성하지 않는다. 닫기 버튼은 제스처가 없는 별도 형제 컨트롤로 유지한다. 한 세션의 순서 스냅샷을 Rc로 공유하고 실제 행 좌표로 드롭 위치를 찾으며 선 하나를 재사용한다. 창, 행, 공간, 선택, 전체 순서와 배치, 입력기 상태를 native 조회 전후에 검사한다. 목록 변경과 패널, 창 비활성, 크기 변경, Escape와 다른 키, 휠, 사이드바 접기, 종료는 취소 경로에 연결했다. 늦은 이벤트가 취소한 세션을 복원하지 않도록 시작 이벤트의 세대와 현재 세션을 검사한다.

`src/browser.rs`는 `TabDragTarget`, `validate_tab_drag_target`, `reorder_tab_by_drag`로 살아 있는 탭과 원래 공간, 선택과 순서를 검사한다. `src/spaces.rs`의 `reorder_before`는 기존 Vec 구간만 회전한다. 같은 위치는 무변경이며 드롭 시에만 반영한다. 배경 행을 옮겨도 현재 선택과 탐색 세대, AI 소유권과 revision을 바꾸지 않는다. CEF나 AppKit 호출, 포커스와 handoff를 순서 변경 사이에 넣지 않는다. 성공 후 선택한 탭으로 강제 스크롤하는 요청도 추가하지 않았다.

Cargo는 기존 objc2-app-kit의 `NSGestureRecognizer` 기능만 활성화했고 새 패키지나 런타임 프로세스는 추가하지 않았다. README, 아키텍처, 제품 계획, 메모리와 `docs/tab-order.md`에 계약과 한계를 기록했다. 현재 구현은 시작 당시 보이는 스크롤 영역 안에서만 이동한다. 영역 이탈과 스크롤은 취소하며 가장자리 자동 스크롤, 화면 밖 위치까지 이어지는 드래그는 남은 작업이다. 이 제한을 독립 브라우저 완성으로 바꾸어 설명하지 않는다.

검증 결과는 다음과 같이 구분한다.

- 최초 tab-drag-v1 컴파일은 `NSButton` 참조를 `NSView` 포인터와 직접 비교한 타입 불일치로 실패했다. 실제 상위 view 참조를 명시하도록 수정한 최종 tab-drag-v2에서 오프라인 잠금 의존성 단위 실행 파일 빌드, 기본 구성과 CLI 전용 구성의 전체 타깃 컴파일, format이 통과했다. 최초 로그를 보존했다.
- 최종 Rust 단위 282개 전부 통과. 신규 9개는 공간 모델 2개, 브라우저 상태 3개, 순수 UI 정책 4개다. 임의 경계 이동과 무변경, 선택과 배열 용량 보존, 오래된 공간과 순서 거절, 배경 AI 탭의 소유권과 탐색 세대 보존, 잘못된 드롭, 위치 계산과 6pt 경계를 확인한다. 비활성 CEF handle과 순수 상태 검사이며 실제 엔진이나 AppKit 통합 실행은 아니다. 기존 Unix 소켓 검사 때문에 단위 실행 파일만 별도 권한으로 실행했다. 검사 전후 전체 src 해시가 같다.
- 기존 단위 dead_code 경고 3개, CLI 라이브러리 경고 9개와 CLI 단위 구성 get 경고 1개는 남는다. `artifacts/tab-drag-v1-check.json`, `tab-drag-v2-check.json`과 구성별 로그가 근거다. 독립 읽기 교차 리뷰에서 추가 확정 P1, P2 결함은 발견하지 못했다. `git diff --check`와 변경 문서의 로컬 링크 대상 확인도 통과했다.
- 분리된 AppKit 실험은 자체 NSWindow와 NSButton에 앱 내부 합성 NSEvent를 게시했다. 최초 제한 환경은 NSApplication 초기화의 XPC 연결 오류로 12초 후 해당 PID만 종료했다. 권한을 조정한 기본 NSPan 검사에서는 일반 클릭, 드래그, 드래그 뒤 클릭은 통과했지만 작은 이동도 드래그로 인식해 클릭이 사라졌다. 6pt를 검사하는 직접 하위 클래스로 바꾼 v2는 지정한 5개 사례를 통과했다. 기본 위치 조회가 원점을 반환해 v3부터 이벤트 window 좌표를 보관하고 view 좌표로 변환했으며 이벤트 19개, action 11개의 좌표 일치를 확인했다. 이 결과를 최종 Naru나 실제 물리 입력 검증으로 확대하지 않는다.
- 추가 Escape 실험은 합성 키 이벤트가 recognizer뿐 아니라 NSApplication의 sendEvent에도 도달하지 않았다. v5에서는 테스트 앱을 활성화했어도 같았다. v3에서 recognizer 전달 문제로 추정했던 해석은 정정했다. 제품 취소 결함을 증명한 결과가 아니며 실제 Escape 검증은 남는다. 마지막 v5도 일반 클릭과 드래그 계열 6개는 통과했지만 Escape 1개 때문에 전체 exit 1이었다. 실패를 성공으로 집계하지 않는다. `artifacts/tab-drag-native-probe/summary-v2-v3.json`, `summary-v4-v5.json`과 버전별 원문 로그에 경계를 보존했다.
- DOM bridge는 변경하지 않아 모의 DOM 검사는 다시 실행하지 않았다. 최신 Naru 앱의 로컬 CEF, 드래그와 메뉴 통합, 실제 Escape, 주소 초안과 입력기, 접근성, 외부 사이트, 장시간 부하와 RAM 측정은 미실행이다. 제품 앱 번들과 서명을 만들거나 CEF를 초기화하지 않았으며 키체인과 사용자 프로필에 접근하지 않았다. 독립 브라우저 전체와 Preseller 내장은 계속 미완료다.

컴파일과 단위 검사 세션은 모두 종료됐다. 격리 `target/.tab-drag-test-u7mlz2wo`의 파일 2,060개, 크기 합계 1,472,137,747바이트, 약 1.37GiB와 symlink 0개를 확인한 뒤 해당 경로만 제거했다. 기존 앱과 CLI, src 해시는 정리 전후 동일하다. 분리된 AppKit 검사 PID 8234, 8608, 9308, 9750, 10825, 12238은 모두 부재를 확인했고 테스트 실행파일 합계 407,736바이트를 제거했다. 마지막 테스트가 잠깐 활성화한 창을 종료한 뒤 이전 Codex 앱 복귀도 확인했다. 브라우저 앱과 서버는 시작하지 않았으며 기존 Naru 프로세스 상태는 다시 조회하지 않았다. 검증 소스와 로그는 Git에서 제외한 artifacts에 보존했다. 디스크 파일 크기 합계를 실제 RAM 절감이나 물리 디스크 회수량으로 설명하지 않는다.

## 2026-10-15, 링크 주소 새 탭 열기와 현재 변경 커밋·푸시

사용자가 커밋에 이어 푸시까지 요청했다. 이번 범위는 앞서 미커밋으로 남긴 탭 순서 변경, 우클릭 메뉴 제어권·프레임 보호와 이번 링크 주소 열기 및 관련 문서 11개를 기존 `codex/navigation-readiness` 브랜치에 커밋하고 `origin`으로 푸시하는 것이다. GitHub 조회에서 `FEABE/naru-browser`의 `isPrivate: true`를 확인했다. 작업 전 원격 브랜치와 로컬 HEAD는 `63c94a4ef0b9954560538468e1165acfbc18ee1c`로 같았다. 최초 네트워크 제한으로 조회가 실패해 읽기 전용 권한으로 다시 확인했다. 기능과 문서를 `35650c689f664fe38fc25f5d7b6e1cf05e1622dc`로 커밋했고 기존 원격 브랜치 푸시가 성공했다. 이후 `git ls-remote`에서 같은 커밋을 확인했으며 워크트리도 깨끗했다. 이 확인 결과를 작업 기록에 추가해 후속 문서 커밋으로 함께 푸시한다. 기본 브랜치 변경이나 배포는 하지 않았다.

`src/browser.rs`는 유효한 HTTP(S) 링크의 네이티브 우클릭 메뉴에 ‘링크 주소를 새 탭에서 열기’를 추가한다. CEF의 필터된 링크 URL을 사용하며 복사 전 UTF-16 길이를 제한하고 기존 주소 정규화의 8,192바이트 상한, 자격증명·제어문자 거절을 적용한다. 메뉴와 페이지·프레임·소유권을 재검사한 뒤 고정 URL, 인계 후 소유권과 원래 공간을 기존 생성 큐로 넘긴다. 같은 공간의 AI 비소유 배경 탭으로 열며 현재 선택을 유지한다. 정상 메뉴 종료와 이후 문서 이동은 수락한 주소를 바꾸지 않는다. 원본 닫힘, 공간과 소유권·revision 변경은 큐와 생성 완료 시 거절한다. `src/control_panel.rs`는 큐·생성 실패를 기존 상태 표시줄에 알리며 이미 중지한 AI를 자동 재개하지 않는다.

기존 URL 생성 경로를 재사용하므로 원래 링크의 JavaScript, download 속성과 referrer·opener 연결을 재현하지 않는다. CEF 고정 원문으로 이 한계를 확인했다. 새 worker, UI renderer와 저장 파일은 없다. 사용자 새 탭에는 기존 자동화 탭 예산을 적용하지 않으므로 Chromium 전체 RAM이나 사용자 생성 큐의 개수 상한을 보장하지 않는다. README, 아키텍처, 제품 계획, 메모리와 메뉴 설계 문서에 계약을 반영했다. Preseller 문서는 이미 구현된 접근 가능 iframe 합성 입력의 모의 검사 범위를 정정했으며 Preseller 소스를 새로 읽거나 실행한 작업은 아니다.

검증 결과는 다음과 같이 구분한다.

- context-link-v1은 Rust 단위 272개 통과, 1개 실패였다. 실패는 AI 공간에 있는 상태에서 사용자 공간의 탭을 공간 전환 없이 선택하고 실패 반환을 확인하지 않은 테스트 설정 오류였다. 제품 코드는 바꾸지 않고 테스트에 사용자 공간 전환과 선택 성공 검사를 추가했다. 최초 실패 로그를 보존했다.
- 최종 context-link-v2는 Rust 단위 273개가 모두 통과했다. 링크 관련 신규 5개는 URL·길이·정규화, getter와 메뉴 변경 중 재진입, 인계 후 소유권과 정상 메뉴 종료, 원본 공간·소유권·닫힘, 큐 실패와 단발 실행을 검사한다. 실제 Rust 상태와 주입한 조회·게시 함수의 결과이며 실제 CEF 메뉴 객체나 엔진을 실행한 결과는 아니다. 기존 Unix 소켓 검사를 위해 컴파일된 단위 실행 파일만 별도 권한으로 실행했다.
- 오프라인 잠금 의존성의 `cargo test --lib --no-run`, 기본 구성 및 CLI 전용 구성의 `cargo check --all-targets`, `cargo fmt --all --check`가 모두 통과했다. 검사 전후 전체 src 해시가 같았다. 기존 단위 구성 dead_code 3개, CLI 라이브러리 9개와 CLI 단위 구성 get 경고 1개는 남는다. `artifacts/context-link-v1-check.json`, `context-link-v2-check.json`과 각각의 로그가 근거다. 읽기 전용 독립 코드 검토에서 추가 확정 P1/P2 결함은 발견하지 못했다.
- 커밋 후보는 Rust와 Markdown뿐이며 생성물·프로필은 추적하지 않는다. 공통 토큰·개인키·비밀번호 할당 패턴 검사에서 실제 비밀값을 발견하지 못했다. 새 설계 문서 두 개도 포함한다. 이전 기록의 미커밋·미푸시는 각 작업 시점의 상태이며 이번 사용자 요청으로 커밋·푸시 범위에 포함한다.
- DOM bridge를 변경하지 않아 모의 DOM 검사는 다시 실행하지 않았다. 로컬 CEF 통합, 실제 AppKit 메뉴와 포커스, 외부 사이트·네트워크 요청 헤더, 장시간 사용과 RAM 측정은 미실행이다. 새 앱 번들·서명을 만들거나 앱과 엔진을 실행하지 않았고 사용자 프로필·키체인에 접근하지 않았다. 최신 실제 앱 검증과 독립 브라우저 전체 완성, Preseller 내장은 남아 있으며 재시작 탭 복원 보류도 유지한다.

v1과 v2의 빌드·검사 세션은 모두 종료됐다. 두 실행이 공유한 `target/.context-link-test-k7tm6y5h`의 파일 2,353개, 크기 합계 1,623,753,946바이트(약 1.51GiB), symlink 0개를 확인한 뒤 해당 경로만 제거하고 부재를 확인했다. 물리 디스크 회수량은 측정하지 않았다. 기존 앱과 CLI, src 해시는 정리 전후 같았다. 이번 작업에서 시작한 앱·서버가 없으며 기존 프로세스 상태는 다시 조회하지 않았다. 검증 로그는 Git에서 제외한 artifacts에 보존하고 사용자 데이터와 기존 번들은 유지했다. 배포는 하지 않는다.

## 2026-10-15, 우클릭 메뉴의 제어권과 프레임 보호 소스

독립 Naru에서 사용자 입력과 AI 제어권이 겹치는 경로를 보완했다. 직전 푸시 요청은 원격 커밋 일치 확인까지 완료했고, 이후 목표 진행에서 작업 중이던 메뉴 보호를 마무리했다. `src/browser.rs`, `src/control_panel.rs`, README, 아키텍처, 제품 계획과 메모리 문서를 변경했으며 `docs/context-menu-control.md`에 고정 엔진 근거와 수명을 기록했다. 이번 개발 변경은 미커밋이며 추가 푸시와 배포를 하지 않았다.

앞선 읽기 전용 실행 환경 조회에서 명시적 `NARU_CODESIGN_IDENTITY`가 없었고 현재 도구 환경의 `security find-identity -v -p codesigning`은 유효 항목 0개를 반환했다. Naru와 SecurityAgent 프로세스 조회는 샌드박스 제한 후 읽기 전용 권한으로 재시도해 일치 항목이 없었다. 이 결과를 시스템 전체 인증서의 부재로 확대하지 않는다. `artifacts/context-menu-v1-environment.json`에 범위를 남겼고 새 인증서 생성, 앱 자동 실행이나 키체인 승인 요청을 하지 않았다.

본문 우클릭은 기존 입력 경로에서 AI를 중지하지만 CEF 기본 메뉴는 `itemSelected:`로 명령을 실행하므로 열린 뒤 작업이 재개되면 일반 편집 selector 검사만으로 다시 보호되지 않는 소스 경로를 확인했다. 설치 CEF 헤더와 고정 CEF, Chromium 원문에서 메뉴 중 앱 작업 처리, 기본 명령 전 handler 검사, 명령 뒤 게시되는 종료 알림을 확인했다. 메뉴가 실제로 재개 뒤 살아남아 오작동하는 현상을 재현한 결과는 아니다.

탭에 값만 가진 메뉴 대상 하나와 generation을 보관한다. 선택, 공간, 탐색과 메인 문서 세대, 소유 작업의 revision과 상태를 명령 직전에 검사하고 대상을 한 번 소비한다. native getter와 handoff 호출 사이에 STATE나 SHELL borrow를 유지하지 않으며 반환 뒤 상태를 다시 검사한다. 활성 작업에는 기존 인계를 호출하고 자체 인계가 만든 epoch 1 증가, revision 1 증가와 paused 전환만 허용한다. 메뉴를 연 뒤 재개된 작업은 다시 중지하지 않고 이전 명령을 거절한다. 같은 페이지가 보이면 기존 상태 표시줄에 재열기 안내를 남긴다.

원문 리뷰에서 CEF 편집 명령이 원본 프레임을 고정하지 않고 실행 당시 focused frame을 사용한다는 경계를 추가로 확인했다. v2는 콜백의 유효하고 비어 있지 않은 프레임 ID를 캡처하고 UI 검사 후 현재 focused frame ID를 조회한다. getter와 handoff 뒤 상태를 다시 검사하며, UI 검사 중 닫힌 탭의 Browser를 사용하지 않도록 현재 상태에서 조회한다. 모든 frame의 before-browse와 load-start에서 메뉴를 무효화한다. 다른 iframe의 탐색도 메뉴를 다시 열게 할 수 있는 보수적인 정책이다. 문서 내부 DOM 변경 전체와 이미 전달된 편집 동작의 취소를 보장하지 않는다.

검증 결과는 다음과 같이 구분한다.

- v1은 Rust 단위 265개, 기본 구성과 CLI 전용 구성의 전체 타깃 컴파일, format이 통과했다. 이후 원문에서 확인한 프레임 경계를 보완했으므로 이 결과를 최종 소스 검증으로 대신하지 않는다. `artifacts/context-menu-v1-check.json`과 구성별 로그에 보존했다.
- 최종 v2는 Rust 단위 268개 전부 통과했다. 앞선 탭 순서 시점 259개에서 이번 9개를 추가했다. 사용자와 paused 메뉴의 단발 실행, 해당 active 작업만 인계, resume와 소유권 변경, 선택과 문서, 닫힘, UI 대상과 재진입, 프레임 불일치와 부재를 확인했다. 하위 프레임 검사는 비활성 CEF handle로 실제 browse와 load-start handler를 호출해 메인 세대 불변과 메뉴 거절을 확인한다. 실제 CEF 엔진과 프레임 getter, AppKit 메뉴의 실행 증거는 아니다. 기존 로컬 Unix 소켓 검사 때문에 컴파일한 단위 실행 파일만 별도 권한으로 실행했다.
- v2의 오프라인 `cargo test --lib --no-run`, 기본 구성과 CLI 전용 구성의 `cargo check --all-targets`, `cargo fmt --all --check`, `git diff --check`가 통과했다. 기존 단위 구성 dead_code 3개, CLI 라이브러리 9개와 CLI 단위 구성 get 경고 1개는 남는다. 검사 전후 전체 src 해시가 동일하다. `artifacts/context-menu-v2-check.json`, `context-menu-v2-rust-tests.log`와 구성별 로그가 근거다. 독립 읽기 리뷰에서 추가 확정 P1, P2는 발견하지 못했다.
- DOM bridge를 바꾸지 않아 모의 DOM 검사는 다시 실행하지 않았다. 로컬 HTTP와 CEF 통합, 실제 우클릭과 Copy, Paste, 키보드, iframe 경합, AppKit key-window 상태와 IME, 외부 사이트, 장시간 사용과 RAM 비교는 미실행이다. 새 번들과 서명을 만들거나 앱을 실행하지 않았고 사용자 프로필에 접근하지 않았다. 최신 네이티브 검증과 독립 브라우저 전체 완성, Preseller 내장은 남아 있다.

컴파일과 단위 실행 세션은 모두 종료됐다. v1과 v2는 같은 격리 target을 재사용했다. 확인한 `target/.context-menu-test-47kbdjpa`의 파일 2,353개, 크기 합계 1,619,232,575바이트, 약 1.51GiB와 symlink 0개를 기준으로 해당 경로만 제거하고 부재를 확인했다. 물리 디스크 회수량은 별도 측정하지 않았다. 기존 앱과 CLI 해시는 검사와 정리 전후 동일하며 새 앱과 서버가 없어 추가 종료 대상도 없다. 새 worker, 타이머, UI renderer나 저장 파일은 제품에 추가하지 않았다.

## 2026-10-15, 추가 푸시 요청과 원격 커밋 일치 확인

사용자가 푸시까지 요청해 기존 작업 브랜치 `codex/navigation-readiness`의 커밋을 `FEABE/naru-browser`로 푸시했다. GitHub API에서 `isPrivate: true`를 확인했고 `git push origin HEAD:refs/heads/codex/navigation-readiness`는 성공 종료하며 `Everything up-to-date`를 반환했다. 이후 `git ls-remote`의 원격 브랜치와 로컬 HEAD가 모두 `63c94a4ef0b9954560538468e1165acfbc18ee1c`임을 확인했다. 기본 브랜치 변경과 배포는 하지 않았다.

이미 완료된 커밋 이후 작업 중이던 탭 순서 변경과 네이티브 컨텍스트 메뉴 제어권 보호 소스는 미커밋으로 남겼다. 추가 기능 개발은 멈췄으며 이번 푸시에는 포함하지 않았다. 컨텍스트 메뉴 변경은 `src/browser.rs`, `src/control_panel.rs`에 저장돼 있지만 컴파일과 단위 실행, 고정 CEF의 메뉴 종료와 명령 콜백 순서, 실제 네이티브 동작 확인이 남아 있다. 탭 순서 검사 결과는 아래 별도 기록의 소스 시점에만 해당한다.

이번 요청에서는 Git 상태와 원격 공개 범위, 푸시 결과와 커밋 일치를 확인했다. 처음 GitHub 조회는 네트워크 제한으로 실패했고 승인된 도구 권한으로 다시 조회해 확인했다. 기능 테스트, 모의 DOM, 로컬 CEF, 네이티브 UI와 외부 사이트 검사는 실행하지 않았다. 앱, 빌드와 서버를 시작하지 않아 새 정리 대상은 없다. 사용자 프로필과 키체인에 접근하지 않았다. 이 작업 기록도 미커밋으로 남기며 현재 작업 중인 소스를 검증 완료로 간주하지 않는다.

## 2026-10-15, 같은 공간의 현재 탭 순서 변경 소스

활성 목표인 Aside의 일상 사용성과 ego lite의 사용자·AI 공간을 결합하는 후속으로 탭 순서 조작을 추가했다. 앞선 커밋·푸시 요청은 완료됐으며 이번 개발 변경은 별도 미커밋 상태다. 페이지 확대는 공유 host 설정이 다른 AI 탭에 영향을 줄 수 있는 기존 보류 사유를 유지했다. 북마크 복구는 손상 원본의 보존 위치 결정이 없어 구현하지 않았고, 사용자가 보류한 재시작 탭 복원도 추가하지 않았다.

`src/spaces.rs`에 공간별 실행 중 탭 표시 순서를 두고 `ordered_tabs`, 방향과 실제 이웃을 검증하는 `reorder_adjacent`를 추가했다. 신규 삽입, 중복 등록, 입양·반환, 닫기와 완료 공간 정리를 같은 순서에 반영한다. 새 탭과 공간 이동 탭은 목적지 마지막에 놓는다. 현재 탭이 사라지면 가장 낮은 ID 대신 표시 순서의 첫 탭을 선택한다. `src/browser.rs`는 SpaceView와 shell snapshot 순서를 일치시키고 실제 선택, 소속, 탭 수명과 방향별 이웃을 재검증한 뒤 메타데이터만 교체한다.

`src/control_panel.rs`의 보기 메뉴에 현재 탭 위로 이동·아래로 이동을 추가했다. 열린 메뉴는 대상 탭과 이웃, 공간과 방향을 고정하며, validation에서 새 선택으로 바꾸지 않는다. 한 탭, 경계, 전체 패널·공간 보기, 비활성 창과 IME 조합은 거절한다. 기존 지연 UI 갱신과 탭 행을 재사용하고 선택한 페이지, 주소 초안, 탐색 세대나 AI 소유권 변경을 직접 요청하지 않는다. 번호 선택과 이전·다음 탭도 같은 표시 순서를 따른다. 실제 AppKit 이벤트와 포커스 확인은 아래 미실행 범위에 남긴다.

공간당 탭 ID Vec 하나와 메뉴 대상 최대 두 개를 추가했다. 빈 배열은 해제하고 capacity가 32를 넘으며 길이가 1/4 이하가 됐을 때만 길이의 두 배를 기준으로 축소 요청한다. 새 worker, 타이머, 웹 renderer, 이미지 캐시나 저장 파일은 없다. `docs/tab-order.md`, README, 아키텍처, 제품 계획과 메모리 문서에 동작, 종료 후 미보존과 자원 경계를 기록했다. 드래그 정렬, CLI 순서 변경 명령과 공간 간 드래그 이동은 미구현이다.

검증은 다음과 같이 구분한다.

- Rust 단위 259개가 전부 통과했다. 새 8개는 공간 순서와 삽입·중복·삭제·입양·완료 수명, 방향과 오래된 대상 거절, 128개에서 줄어든 배열 용량 회수, 가짜 CEF handle 상태에서 실제 shell snapshot 순서와 AI 소유권·선택·탐색 세대 보존, 순수 메뉴 대상과 번호·순환 순서를 확인한다. 실제 CEF 엔진이나 AppKit 메뉴를 실행한 증거는 아니다. 기존 로컬 Unix 소켓 검사를 위해 컴파일된 단위 실행 파일만 별도 권한으로 실행했다.
- 오프라인 `cargo test --lib --no-run`, 기본 구성과 CLI 전용 구성의 `cargo check --all-targets`, `cargo fmt --all --check`, `git diff --check`가 통과했다. 기존 단위 구성 dead_code 3개, CLI 라이브러리 9개와 CLI 단위 구성 get 경고 1개는 남는다. 검사 전후 전체 src 해시가 동일하다. `artifacts/tab-order-v1-check.json`, `tab-order-v1-rust-tests.log`와 구성별 로그가 근거다.
- DOM bridge를 변경하지 않아 모의 DOM 검사는 다시 실행하지 않았다. 로컬 HTTP·CEF 통합, 네이티브 메뉴의 활성화와 추적·키보드·주소 편집·IME·접근성·사이드바 스크롤, 외부 사이트, 장시간 사용과 RAM 비교는 실행하지 않았다. 반복 키체인 승인 요청을 하지 않으며 기존 번들을 교체하거나 새 앱을 실행하지 않았다. 독립 소스 리뷰에서 추가 확정 P1/P2는 발견하지 못했지만 실제 메뉴 조작 성공으로 표시하지 않는다.
- 탐색 보조로 src 29파일의 graphify AST 지도를 `artifacts/graphify-src-20261015/graphify-out`에 만들었다. 최초 stdin multiprocessing은 실패했고 설치된 도구의 직렬 추출로 성공했다. 1,437개 노드·4,600개 연결이며 dangling 79개와 같은 endpoint 병합 500개의 한계가 있어 호출 계약 증거로 사용하지 않았다. 이번 수정 전 지도이고 재생성하지 않았다. 코드 호출 경로는 실제 소스에서 별도로 확인했다.

컴파일과 단위 검사 세션은 종료됐다. 격리 `target/.tab-order-test-fiiydt50`의 파일 2,060개, 크기 합계 1,460,617,728바이트(약 1.36GiB)를 제거하고 경로 부재를 확인했다. 물리 디스크 회수량은 별도 측정하지 않았다. 기존 앱과 CLI 해시는 전후 동일하며 사용자 프로필과 탭에 접근하지 않았다. 새 앱·서버와 키체인 요청이 없고 기존 프로세스 상태는 재조회하지 않았다. 독립 브라우저 전체, 최종 앱 검증과 Preseller 내장은 미완료이며 이번 변경을 커밋·푸시·배포하지 않았다.

## 2026-10-15, 사용자 추가 요청에 따른 비공개 원격 푸시 준비

사용자가 로컬 커밋에 이어 푸시도 요청했다. 기능 커밋 `eb39c53`과 이 기록을 기존 `origin/codex/navigation-readiness`에 올리는 범위다. 원격 주소가 `https://github.com/FEABE/naru-browser.git`이고 GitHub 조회의 `isPrivate`가 true임을 확인했다. 작업 전 워크트리는 깨끗했고 로컬 브랜치는 추적 브랜치보다 한 커밋 앞서 있었다. 이번 변경 파일은 `docs/work-log.md`뿐이다. 푸시 결과와 원격 커밋 일치는 실행 후 Git 조회로 확인해 보고한다.

이 요청에서는 기능 소스를 바꾸거나 테스트를 다시 실행하지 않았다. 최신 소스의 실제 CEF, 네이티브 UI, 외부 사이트와 RAM 검증은 앞선 기록대로 미완료이며 탭 복원은 보류 상태다. 앱, 빌드나 서버를 실행하지 않아 정리할 새 프로세스와 빌드 생성물이 없다. 배포는 포함하지 않는다.

## 2026-10-15, 사용자 요청에 따른 로컬 커밋 준비

사용자가 커밋만 요청했다. 현재 `codex/navigation-readiness` 브랜치의 Naru 소스, 문서와 테스트 변경 39개를 한 커밋으로 남기는 범위이며 푸시와 배포는 포함하지 않는다. 별도 기능 수정은 하지 않고 이 작업 기록을 추가했다. 주요 변경은 네이티브 시작 화면과 탐색 복구, 주소와 탭 입력 보호, 방문 기록, 다운로드 관련 탭과 사이트 설정, 현재 탭 음소거, 접근 가능한 iframe의 CLI 관찰과 입력이다. 탭 복원은 사용자 보류 상태의 검토 문서만 포함한다.

커밋 전 git 상태와 변경 목록, 파일 형식과 크기, 일반적인 자격 증명 패턴을 검사했다. 후보는 UTF-8 일반 파일이며 생성물, 프로필과 환경 파일은 포함하지 않는다. 패턴 검사에서 발견된 자격 증명은 없으나 모든 비밀값의 부재를 증명하는 검사는 아니다. 기존 stage는 비어 있었다. `git diff --check`가 통과했고 `artifacts/iframe-v1-source-sha256.json`의 전체 src와 변경 DOM, CEF fixture 테스트 해시가 현재 파일과 일치한다. 직전 근거는 모의 DOM 55개, Rust 단위 251개와 두 구성 전체 타깃 검사 통과다. 이번 커밋 요청에서는 테스트를 다시 실행하지 않았다.

최신 소스의 실제 CEF, 네이티브 UI, 외부 사이트와 RAM 검증은 여전히 미완료다. 기존 앱을 새 소스로 교체하지 않았다. 새 빌드나 앱, 서버, 프로필 접근과 키체인 요청이 없어 정리할 새 실행 자원도 없다. 이전 격리 빌드 정리 결과는 바로 아래 작업 기록을 따른다. 커밋 식별자와 최종 워크트리 상태는 커밋 후 Git 조회 결과로 보고한다.

## 2026-10-15, 탭 복원 보류와 접근 가능한 iframe 제어 소스

제품 계획의 재시작 탭 복원부터 현재 호출 흐름을 조사했다. 별도 JSON, 내 공간의 최대 16개 URL과 제목, 순서와 선택, 256KiB, 선택한 탭만 먼저 로드하는 검토안을 문서로 만들었지만 사용자가 탭 복원 보류를 선택했다. `docs/session-restore-design.md`에 보류 상태를 명시했으며 세션 저장소, 복원 코드, 지연 탭은 추가하지 않았다.

마지막 탭 닫기와 앱 종료도 조사했다. 현재 do_close가 마지막 탭이면 shell을 해제하고 OnBeforeClose가 메시지 루프를 종료한다. 빈 창 UI는 이미 있지만 기본 beforeunload의 취소 결과를 CEF 공개 콜백에서 구분할 수 없었다. 고정 CEF에서 기본 dialog를 사용하면 handler 연결을 지우는 경로와 TryCloseBrowser의 대기, 취소 모호성을 확인했다. 종료 플래그 하나로 처리하면 취소 뒤 앱을 잠그거나 이후의 다른 닫힘을 종료로 오인할 수 있어 적용하지 않았다. 현재 수명주기는 유지하며 이 조사도 복원 검토안에 기록했다.

이후 독립 브라우저의 AI 관찰 공백인 iframe을 보완했다. `src/dom_bridge.js`에서 메인 문서가 접근할 수 있는 같은 출처 iframe의 본문과 조작 요소, 내부 열린 Shadow DOM을 기존 read와 ref 동작에 연결했다. `interactive` 평면 배열과 Rust의 native epoch 접두어, CLI 형식과 작업 제어권 검사를 유지하고 프레임 내부 항목에만 frame ID를 추가한다. `page.frames`는 read 한 번의 ID와 부모, label, accessible 또는 unavailable 또는 limit 상태를 반환한다. 접근한 문서만 실제 URL과 제목을 제한해서 반환하며 선언 src를 실제 문서 URL로 추정하지 않는다.

프레임당 문서, 부모 iframe과 부모 문서의 연결을 고정한다. action 전 원래 ownerDocument와 contentDocument identity를 확인하고 제거, 교체나 접근 불가 전환을 거절한다. iframe의 크기와 상위 hidden, inert, aria-hidden, 비활성 상태를 검사한다. focus 뒤 바뀐 대상은 쓰기 전에 거절하고 input이나 keydown이 이미 전달된 뒤 바뀐 경우에는 부분 동작을 명시하고 남은 change, 폼 제출과 keyup을 중단한다. child 문서의 Range, style과 isolated realm의 native setter, Event 생성자를 사용한다. 고정 Chromium의 WindowProxy, 문서 바인딩과 보안 검사 원문을 확인했고 근거 링크는 `docs/iframe-control.md`에 남겼다.

기존 본문 12,000자, 요소 120개, 방문 노드 10,000개, 열린 shadow root 64개, 순회 깊이 64와 옵션 240개 예산은 모든 문서가 공유한다. 프레임 metadata 슬롯 16개, 중첩 8단계 제한을 contentDocument getter 전에 적용한다. 접근 불가 항목도 슬롯을 사용한다. child isolated context는 지연 생성될 수 있고 반복 read가 다른 프레임을 방문하면 더 생성될 수 있어 전체 context나 RAM이 16개로 제한된다는 뜻은 아니다. 새 worker, 타이머, renderer와 파일 저장은 없다.

검증과 준비 결과는 다음과 같다.

- 모의 DOM 55개 전부 통과. 이전 46개에 독립 document와 Window double을 이용한 9개를 추가했다. 중첩 iframe과 shadow label, 문서별 중복 ID, child setter와 이벤트 생성자, 비밀번호 제외, 프레임 제거와 교체, 접근 예외, 부모 숨김과 0 크기, focus와 keydown 중 변경, input 뒤 부분 동작, 원문서에서 다른 문서로 이동한 요소 거절, read 세대와 전역 한도를 검사했다. `artifacts/iframe-v1-dom-tests.log`가 근거다. 실제 브라우저 SOP나 realm 동작의 실행 증거는 아니다.
- 첫 모의 DOM 실행은 54개 통과, 1개 실패했다. 테스트 예외 문자열 inaccessible이 정상 limitations 문구와 겹친 오탐이었다. 고유 sentinel로 바꾸고 상한 이후 getter 호출 검사도 잡힐 수 있는 예외 대신 카운터로 확인하도록 강화했다. 제품 소스 변경 없이 전체 55개가 통과했다. 초기 로그는 `iframe-v1-dom-tests-initial.log`에 보존했다.
- Rust 단위 251개 전부 통과했고 신규 Rust 테스트는 없다. 기본 기능 전체 타깃과 browser 기능을 끈 CLI 전체 타깃 Cargo 검사, cargo fmt와 git diff 검사도 통과했다. 캐시된 CEF와 Ninja를 사용해 오프라인으로 검사했으며 단위 실행 파일만 기존 Unix 소켓 검사 권한으로 실행했다. 기존 단위 구성 dead_code 3개, CLI 라이브러리 9개와 CLI 단위 구성 get 경고 1개가 남는다. 검사 전후 전체 src와 변경 테스트 해시가 동일하다.
- `tests/shadow_smoke.py`에 실제 CEF에서 child 입력, select, Enter와 중첩 shadow 클릭, 비밀번호 제외, frame 제거 후 stale ref를 확인할 fixture를 추가했다. 기존 제외용 iframe은 opaque sandbox로 명시해 새 접근 정책에 맞췄다. Python AST, 내장 fixture JavaScript와 변경 JS 문법 검사는 통과했지만 이 통합 검사를 실행하지 않았다.
- 독립 읽기 리뷰에서 추가 확정 P1, P2는 발견하지 못했다. 실제 CEF 엔진과 HTTP, AppKit 입력과 IME, 프레임 탐색 및 renderer 경쟁, 외부 사이트, 장시간 사용과 RAM 비교는 실행하지 않았다. 앱 번들 생성과 서명, 앱 실행, 프로필 접근과 키체인 요청을 하지 않았다.

README, 아키텍처, 제품 계획과 메모리 문서를 현재 소스 범위로 갱신했다. 실제 final 앱의 다중 realm과 이벤트 동작은 미검증이다. 교차 출처 frame 내용, 프레임 내부 스크롤, native 좌표 입력과 screenshot 정합, 전체 독립 브라우저 완성과 Preseller 내장은 남아 있다. 탭 복원은 사용자 보류 상태를 유지한다.

이번 컴파일과 검사 세션은 모두 종료됐다. 새 앱이나 서버는 시작하지 않았으며 기존 프로세스 상태는 다시 조회하지 않았다. 격리 `target/.iframe-test-0oeownlz`의 파일 1,392개, 파일 크기 합계 836,733,616바이트, 약 798MiB를 제거하고 경로 부재를 확인했다. 물리 디스크 블록 회수량 측정은 아니다. 기존 앱과 CLI 해시는 전후 동일하고 사용자 프로필과 탭은 건드리지 않았다. 종합 근거는 `artifacts/iframe-v1-validation.json`이다. 커밋, 푸시와 배포는 하지 않았다.

## 2026-10-15, 현재 탭 음소거 소스

독립 Naru의 일상 탐색을 보완하며 `src/browser.rs`와 `src/control_panel.rs`에 현재 탭 음소거를 추가했다. 보기 메뉴에서 현재 표시한 사용자 또는 AI 탭의 출력 설정을 바꾸며 작업 인계, 선택, 탐색과 주소 초안 변경을 호출하지 않는다. 체크와 기존 탭 아이콘은 CEF에서 확인된 음소거 설정만 표시한다. 현재 소리 재생 여부 감지, 배경 행의 직접 버튼, 영속 설정은 구현하지 않았다.

설치된 CEF 156.0.1 커밋과 Chromium 156.0.8078.17 고정 소스를 읽어 탭별 WebContents 출력 설정, UI 스레드 getter, 기본값과 수명, IO 전달을 확인했다. AudioHandler는 mute_source loopback으로 기본 출력에 영향을 주므로 상태 표시용으로 추가하지 않았다. setter 뒤 getter는 native 설정 반영 확인이며 실제 스피커 무음 완료 증거가 아니다. 근거 링크는 `docs/tab-audio.md`에 남겼다.

메뉴를 열 때 탭, 공간, 탐색 세대와 소유 작업 ID, revision, 상태 및 예상 설정을 캡처한다. 열린 메뉴 validation이 대상을 바꾸지 않으며 action은 한 번 꺼내 실행한다. native getter 전후와 setter 직전에 대상과 UI 상태를 다시 확인하고 예상 설정이 달라지면 쓰지 않는다. setter 중 탭이 닫힌 경우 OnBeforeClose 이후 보관된 host를 재조회하지 않는다. setter 이후 확인 실패는 변경 시도 자체와 구분해 알린다. CEF 호출에 STATE와 SHELL borrow를 유지하지 않는다. 네이티브 창, IME, 오버레이와 renderer 오류 가드도 연결했다.

탭마다 마지막 확인값 Option 하나, 메뉴 대상과 예상값 하나 및 재진입 guard 하나를 추가했다. 메뉴 항목 한 개와 기존 탭 아이콘을 사용한다. 새 PCM 버퍼, worker, 반복 타이머, 웹 UI와 파일 저장은 없다. README, 제품 계획, 메모리 문서와 음소거 설계에 계약과 미검증 범위를 반영했다.

검증 결과는 다음과 같다.

- Rust 단위 251개 전부 통과. 이전 245개에 fake CEF host 검사 4개, 순수 UI 대상과 아이콘 검사 2개를 추가했다. 실제 STATE에서 다른 탭 무변경과 AI 제어권 보존, 문서와 선택, 소유권, renderer 오류와 닫힘에 따른 만료, getter 재진입과 UI veto, 예상 설정 불일치, readback 미반영과 setter 중 닫힘을 검사했다. 엔진 초기화나 AppKit 실행 검사는 아니다. 기존 Unix 소켓 검사를 위해 컴파일된 단위 실행 파일만 승인된 권한으로 실행했다. `artifacts/tab-audio-v1-rust-tests.log`가 근거다.
- 기본 구성 전체 타깃과 browser 기능을 끈 CLI 전체 타깃 Cargo 검사, cargo fmt와 git diff 검사가 통과했다. 캐시된 CEF와 Ninja로 오프라인 컴파일했고 dev와 test 디버그 정보를 끈 격리 target을 사용했다. 기존 단위 구성 dead_code 경고 3개, CLI 라이브러리 경고 9개와 CLI 단위 구성 get 경고 1개가 남는다. 검사 전후 전체 src 해시는 동일하다.
- 독립 읽기 검토에서 추가 P1, P2는 발견하지 못했다. menuWillOpen이 캡처 후 setEnabled를 명시하므로 이전 validation의 disabled가 고착되는 명백한 순서 결함은 보이지 않는다. 실제 AppKit tracking과 validation 순서를 실행한 증거는 아니다.
- DOM 변경이 없어 모의 DOM 검사는 새로 실행하지 않았다. 로컬 HTTP와 CEF 통합, 네이티브 메뉴와 입력, IME와 최소 창, 실제 오디오 출력과 capture, 외부 사이트, 장시간 사용과 RAM 비교는 실행하지 않았다. 앱 번들 생성, 서명, 앱과 CEF 엔진 초기화, 사용자 프로필 접근, 키체인 요청을 하지 않았다. 기존 앱의 검증을 이 소스의 실행 결과로 합산하지 않는다.

실제 메뉴 체크와 음소거, 탐색 중 설정 유지와 새 탭 기본값, 같은 origin의 다른 탭 출력, 주소 초안과 입력 포커스, 숨긴 AI 탭 선택 후 제어권 보존을 최종 앱에서 확인해야 한다. 독립 Naru 전체 완성과 Preseller 내장은 미완료다.

검사 실행 세션은 모두 종료됐다. 새 앱이나 서버를 시작하지 않았으며 기존 앱의 프로세스 상태는 다시 조회하지 않았다. 격리 `target/.tab-audio-test-k5i8poom`의 파일 1,392개와 파일 크기 합계 836,708,523바이트, 약 798MiB를 제거하고 경로 부재를 확인했다. 물리 디스크 블록 회수량은 측정하지 않았다. 기존 앱과 CLI 해시는 작업 전후 같고 사용자 프로필과 탭은 건드리지 않았다. 종합 결과는 `artifacts/tab-audio-v1-validation.json`이다. 커밋, 푸시, 배포는 하지 않았다.

## 2026-10-15, 다운로드와 권한 카드의 관련 탭 이동 소스

독립 Naru의 일상 탐색과 AI 공간 연결을 이어서 보완했다. 실제로 전달된 다운로드 권한 요청은 기존 사이드바에 개수와 강조색으로 표시되지만, 카드에서 해당 공간과 탭을 직접 열 수 없었다. `src/downloads.rs`, `src/download_runtime.rs`, `src/download_permissions.rs`, `src/download_permission_runtime.rs`, `src/browser.rs`, `src/control_panel.rs`를 변경했다. 별도 저장소, 창이나 웹 UI를 추가하지 않았다.

파일 카드에는 접수 탭 보기, 권한 카드에는 요청 탭 보기를 추가했다. 사용자의 명시적 버튼 동작에서만 기존 검색 탭 선택 경로로 공간과 탭을 바꾼다. 권한 요청의 token과 콜백을 조회할 뿐 소비하거나 Continue하지 않는다. 파일 저장, 재시도, 취소나 AI 인계도 호출하지 않는다. 네이티브 저장창이 열렸거나 종료 중이면 이동을 거절하며, 창과 IME, sender 포인터를 확인한다. 실패 전에는 패널과 초안을 지우지 않는다. 성공 뒤 fresh 탭과 공간을 재확인한 경우만 패널을 닫고 포커스를 준다.

파일은 OnBeforeDownload 접수 시 문서와 제어권을 캡처한다. 읽기 검토에서 지연 응답 전 탐색이 있을 수 있어 이를 최초 링크 클릭 문서로 설명할 근거가 없음을 확인했다. 파일 버튼을 접수 탭으로 명시하고 권한의 실제 요청 대상과 구분했다. 접수 이전 원본 문서 추적은 구현하지 않았다. 이후 새 문서와 renderer 종료, 명시적 탐색, 중지, 주소나 제어권 변경은 이전 대상을 거절한다. 탭마다 출처 세대 u64 하나로 소유권 왕복도 무효화한다. attachment OnBeforeBrowse는 현재 문서를 교체하지 않을 수 있어 이 세대를 바꾸지 않는다. 같은 문서의 추가 파일 뒤에도 접수 탭을 볼 수 있는 경계다. 파일 수명과 source target 수명은 분리하며 paused 사용자 파일의 기존 저장 범위는 유지한다.

중지된 AI 공간의 출처는 실제 작업 이름, 사용자 제어와 탭 번호로 표시했다. 작업 이름 48자 제한으로 기존 80자 출처 상한에서 탭 번호가 남게 했다. 파일 최대 24개와 권한 최대 4개 행에 버튼 하나씩을 추가하고 기존 컨트롤 재사용, 닫을 때 해제를 유지한다. 목록이 열려 갱신할 때 최대 24개 대상을 임시 복사하고 RUNTIME borrow 밖에서 순수 browser 상태로 검증한다. 새 알림 큐나 worker, 타이머, URL과 Browser 보관은 없다. 파일 행은 세 버튼을 하단에 배치하며 높이 120pt, 간격 128pt다. 실제 RAM과 화면 변화는 측정하지 않았다.

검증 범위와 결과는 다음과 같다.

- Rust 단위 245개 전부 통과. 이전 239개에 파일 메타데이터와 독립 수명 1개, 파일 런타임 조회와 저장창, 종료 가드 1개, 권한 조회의 무소비와 무승인 1개, 실제 browser STATE의 이동과 만료 1개, UI 선택과 공간 모델 1개, 좁은 폭 버튼 기하 1개를 추가했다. browser 검사는 attachment intent 후 유효한 대상, 사용자 공간에서 AI 탭 선택, revision 보존, 문서와 renderer, 닫힘, 소유권 왕복과 stale 대상의 무변경을 확인했다. 네이티브 UI와 CEF 엔진 실행 검사는 아니다. 기존 Unix 소켓 검사 때문에 단위 실행 파일만 승인된 권한으로 실행했다. `artifacts/download-source-v1-rust-tests.log`가 근거다.
- 기본 기능 전체 타깃과 browser 기능을 끈 CLI 전체 타깃 Cargo 검사, cargo fmt와 git diff 검사를 통과했다. 캐시된 CEF와 Ninja로 오프라인 컴파일했고 테스트와 dev 디버그 정보를 끈 격리 target을 사용했다. 단위 구성의 기존 dead_code 경고 3개, CLI 라이브러리의 미사용 권한 코드 경고 9개와 CLI 단위 구성의 get 경고 1개가 남는다. 검사 전후 전체 src 해시가 같으며 앱 바이너리는 만들지 않았다.
- 독립 읽기 검토에서 대상 캡처 시점, 콜백 무소비, 파일 수명, borrow와 재진입, 선택 후 닫기와 포커스 경계를 확인했다. 추가 P1, P2는 발견하지 못했다. 실제 AppKit 콜백 순서, 저장창이나 IME 재진입의 실행 증거로 세지 않는다.
- DOM 변경이 없어 모의 DOM 검사는 새로 실행하지 않았다. 로컬 HTTP와 CEF, 네이티브 화면과 저장창, IME와 최소 창, 외부 사이트, 장시간 사용과 RAM 비교는 실행하지 않았다. 앱 번들 생성과 서명, 앱과 엔진 초기화, 사용자 프로필 접근, 키체인 승인 요청을 하지 않았다.

README, 다운로드 설계, 제품 계획과 메모리 문서에 계약과 한계를 반영했다. Chromium이 아직 전달하지 않은 숨긴 권한 대기를 감지하는 기능은 아니다. 접수된 파일조차 없거나 대상이 만료되면 이번 경로로 해당 페이지를 찾을 수 없다. 실제 앱에서 숨긴 탭을 표시한 뒤 권한이 전달되는 흐름, 지연 응답 전 탐색, 동일 탭 주소 초안과 입력 포커스, 저장창과 동시 종료 검증은 남는다. 독립 브라우저 전체 완성과 Preseller 내장도 미완료다.

컴파일과 검사 실행 세션은 모두 종료됐다. 새 앱이나 서버를 시작하지 않았으며 기존 앱 프로세스 상태를 다시 조회하지 않았다. 격리 `target/.download-source-test-797q_w2f`의 파일 1,392개, 파일 크기 합계 836,510,875바이트, 약 798MiB를 확인한 뒤 제거하고 경로 부재를 확인했다. 물리 디스크 블록 회수량은 측정하지 않았다. 기존 앱과 CLI 해시는 작업 전후 동일하고 사용자 프로필과 탭은 건드리지 않았다. `artifacts/download-source-v1-validation.json`에 검사, 경고, 미실행과 정리 결과를 남겼다. 커밋, 푸시, 배포는 하지 않았다.

## 2026-10-15, 현재 사이트 다운로드 허용 설정 초기화 소스

Naru의 일반 탐색 후속으로 제품 계획에 남아 있던 다운로드 사이트 설정 UI를 보완했다. 기존 ACCEPT는 사이트 허용을 저장하지만 승인 뒤 대기 token을 제거해 되돌릴 UI가 없었다. `src/download_permissions.rs`, `src/download_permission_runtime.rs`, `src/browser.rs`, `src/control_panel.rs`를 변경했다. 별도 저장소나 승인 목록 캐시는 추가하지 않았고 사용자 프로필에는 접근하지 않았다.

다운로드 패널에 고정 카드 하나를 추가했다. 현재 main frame의 실제 HTTP(S) origin과 적용 설정을 읽고 ALLOW일 때만 `허용 설정 초기화`를 활성화한다. 버튼은 해당 origin의 저장 예외를 DEFAULT로 되돌린다. 모든 공간의 같은 사이트에 적용하며 대기 요청 취소 가능, 열린 페이지 다시 열기, 진행 중 파일 보존을 안내한다. 전체 승인 사이트 목록이나 새 허용, 차단 기능을 구현한 것은 아니다.

설치 CEF hash와 Chromium tag가 일치하는 원문을 확인했다. AUTOMATIC_DOWNLOADS의 origin scope, ASK 기본값, DEFAULT 예외 삭제와 즉시 context 호출, DEFAULT가 조회 실패로도 반환되는 경계를 확인했다. limiter의 observer는 최근 origin만 갱신하므로 저장 예외 초기화를 모든 기존 문서의 즉시 허용 철회로 표시하면 안 된다. IGNORE는 같은 페이지에서 기다리는 다른 origin 콜백도 취소할 수 있다. 웹 도구의 cache miss와 제한 환경의 DNS 실패 뒤 공개 고정 소스만 읽는 네트워크 권한으로 원문을 확보했다. URL과 SHA-256은 `artifacts/download-site-v1-source-evidence.json`, 해석과 범위는 `docs/download-site-settings.md`에 기록했다.

`browser`는 기존 CurrentPageTarget에 document epoch와 owner, revision, 상태를 묶고 실제 주소 조회 전후 검증한다. `reset_site`는 표시 당시 대상, live origin과 ALLOW, 네이티브 창과 IME를 확인한다. 같은 origin pending을 먼저 꺼내 borrow 밖에서 IGNORE한 뒤 실제 주소와 제어권, 종료 상태를 다시 확인한다. 빈 주소나 전역 기본값 변경, stale 버튼은 거절한다. setter는 AUTOMATIC_DOWNLOADS, DEFAULT만 쓰며 ASK 또는 BLOCK 재조회만 성공으로 처리한다. 여전히 ALLOW거나 조회가 불명확하면 초기화 시도와 확인 실패를 구분한다. 취소한 대기가 있으면 뒤의 실패 안내에도 부분효과를 남긴다.

UI는 카드 하나, 라벨 7개와 버튼 하나를 재사용하고 패널이 열린 경우만 조회한다. CEF 조회 뒤 fresh browser snapshot으로 표시 대상을 확인한다. 읽기 검토에서 nested 조회 생략이 빈 조회 결과로 취급돼 정상 카드와 버튼 포커스를 지울 수 있는 경로를 찾았다. `DownloadSiteQuery::Skipped`를 구분해 같은 대상은 유지하고 바뀐 대상이나 실제 조회 실패만 무효화했다. 기존 AI 작업의 선택과 제어권을 바꾸지 않으며 별도 renderer, worker나 타이머는 없다. 실제 자원 증감은 측정하지 않았다.

검증과 실패 이력은 다음과 같다.

- 최종 Rust 단위 239개 전부 통과. 이전 231개에 origin 1개, RequestContext와 pending 4개, browser STATE 1개, UI 대상과 재진입 모델 2개를 추가했다. 정확한 origin과 빈 secondary, 자동 다운로드 타입과 DEFAULT만 쓰기, stale 대상의 무쓰기, readback 실패, scheme과 port별 대기 취소, 재진입, 문서와 소유권 변경을 검사했다. `artifacts/download-site-v1-rust-tests.log`가 근거다.
- 최초 전체 단위 프로세스와 분리한 새 fake CEF 검사 하나는 exit 139였다. 테스트가 엔진을 로드하지 않은 상태에서 `CefString::from(&str)`의 native UTF-16 변환 함수를 호출했다. core가 검증한 `&CefString`을 getter와 setter에 재사용하도록 하고, 테스트는 호출 범위 동안 유지하는 Rust UTF-16 버퍼를 borrowed CEF 문자열로 전달했다. 엔진 초기화 없이 권한 검사 7개가 통과한 뒤 최종 전체 검사도 통과했다. production은 정상 runtime에서 native 문자열을 한 번 생성한다. 최초 실패와 분리 검사 로그를 보존했으며 제품 앱의 크래시를 재현한 결과로 세지 않는다.
- 기본 기능 전체 타깃과 browser 기능을 끈 CLI 구성 전체 타깃 Cargo 검사, cargo fmt, git diff 공백 검사가 통과했다. 최종 검사 전후 src 해시가 같다. 검사 구성의 observe_history, page_find_result, bookmark_completed dead_code 경고 3개, CLI 전용 구성의 미사용 다운로드 권한 코드 경고 9개가 남는다.
- 독립 읽기 리뷰에서 origin, 실제 주소와 상태 검증, pending 취소와 부분효과, 문자열 수명과 UI 재진입 경계를 확인했다. 추가 P1, P2 수정 사항은 남지 않았다. 실제 CEF 엔진, 설정 파일 반영이나 네이티브 포커스의 실행 증거는 아니다.
- DOM 변경이 없어 모의 DOM 검사는 새로 실행하지 않았다. 로컬 HTTP와 실제 CEF, 네이티브 AppKit과 IME, 최소 창 배치, 외부 사이트, 재실행과 RAM 비교는 실행하지 않았다. 반복 승인을 중단하라는 요청에 따라 앱 번들 생성, 서명, 앱과 CEF 엔진 실행, 키체인 접근을 하지 않았다. 기존 앱의 과거 검증을 새 소스 결과로 합산하지 않는다.

설계, README, 제품 계획, 다운로드와 메모리 문서를 갱신했다. 실제 앱의 설정 반영, 재탐색과 재시작, 여러 공간과 iframe, 대기와 진행 파일, 동시 탐색과 종료, 키보드와 IME는 후속 검증이다. 전체 사이트 권한 관리, 모든 기존 문서의 즉시 철회, 독립 Naru 완성과 Preseller 내장은 계속 미완료다.

이번 실행 세션은 모두 종료됐다. 새 앱이나 서버를 시작하지 않았으며 기존 앱 프로세스 상태는 다시 조회하지 않았다. 초기 테스트 중단 시각과 테스트 전용 이름을 대조해 남은 임시 디렉터리 두 개, 파일 네 개와 325바이트를 확인한 뒤 제거했다. 최종 `target/.download-site-test-ppzvn45j`의 파일 3,943개, 파일 크기 합계 2,301,526,970바이트, 약 2.14GiB도 제거했다. 물리 디스크 블록 회수량 측정은 아니다. 기존 앱과 CLI SHA-256은 작업 전후 같고 사용자 프로필과 기존 탭은 건드리지 않았다. 종합 근거는 `artifacts/download-site-v1-validation.json`, 임시 폴더 정리는 `download-site-v1-crash-temp-audit.json`이다. 커밋, 푸시, 배포는 하지 않았다.

## 2026-10-15, 내 공간 방문 기록과 검색 소스 구현

사용자가 별도 JSON, 내 공간만 기록, 최근 30일과 최대 1,000개, 2MiB, 과거 목록 백업 없음이라는 기본안을 승인했다. 이에 맞춰 `src/history.rs`, `src/history_runtime.rs`를 추가하고 `src/browser.rs`, `src/control_panel.rs`, `src/tab_search.rs`, `src/lib.rs`, `src/main.rs`에 연결했다. 실제 사용자 프로필을 읽거나 기록을 수집하지 않았으며 앱 번들에는 아직 반영하지 않았다.

`history::Store`는 안정적인 ID, 정규화된 전체 HTTP(S) URL, 최대 256자 제목과 마지막 방문 시각만 저장한다. 같은 URL은 갱신하며 오래된 항목과 직렬화 크기 상한을 적용한다. 제한된 읽기, 파일 소유권과 권한, symlink와 hardlink, 디렉터리 교체와 외부 변경을 검사한다. pending 파일에 쓰고 파일 동기화, rename, 폴더 동기화로 반영하며 과거 목록 백업이나 남은 pending 자동 승격은 없다. rename 후 폴더 동기화 실패는 반영된 변경의 내구성 경고로 구분한다. 시스템 시각 역행 때 보존 기간이 늘 수 있는 한계를 문서에 명시했다.

파일 작업은 동시에 하나이며 대기는 최대 64개와 URL, 제목 UTF-8 합계 256KiB로 제한한다. 같은 주소의 방문과 제목은 합치고 넘치면 오래된 대기를 제외해 안내한다. 제목만으로 없는 방문 항목을 만들지 않는다. 전체 삭제는 세대를 먼저 바꾸고 진행 중 쓰기, 삭제, 새 세대 쓰기 순서로 처리한다. 삭제 중에는 이전 결과를 숨기며 완료된 파일 교체 뒤에만 처리 중 표시를 해제한다. 실패는 읽기 전용 상태와 오류로 알린다. 시작 시 만료 파일 정리, 완료 알림이 없을 때 접근 시 결과 회수, 정상 종료의 제한된 대기 처리를 연결했다.

CEF 콜백은 실제 main frame의 문서, 최종 주소, 탐색과 문서 세대, HTTP 완료, 로딩 상태, 사용자 공간과 소유권을 다시 확인해 방문을 전달한다. AI 탐색이 release 후 늦게 끝나거나 단독 commit 콜백만 와도 사용자 기록으로 승격하지 않는다. 실패, 중지와 렌더러 종료도 이전 자격을 취소한다. 임시 생성한 북마크, 기록과 현재 주소 복사의 사용자 탭은 최종 검증과 선택 성공 전까지 기록을 보류한다. 생성 중 콜백이 완료된 뒤 실패해도 기록하지 않는 경계를 추가했다. 삭제 당시 열린 문서는 다음 전체 탐색 전까지 SPA와 fragment 이동, 늦은 제목도 기록하지 않는 제한을 둔다.

통합 검색은 Tab, Bookmark, History의 ID 종류를 구분해 전체 40행을 유지하고 앞선 결과와 같은 URL의 기록을 제외한다. 전용 방문 기록 보기, 검색어를 유지하는 모드 전환, 같은 화면의 전체 삭제 확인과 취소를 추가했다. 삭제 요청 수락을 완료로 표시하지 않는다. 숨긴 버튼이나 제거된 행 때문에 포커스가 창으로 돌아오면 검색 입력을 복구하며 유효 편집기는 유지한다. 추가 컨트롤 4개는 고정 생성하고 별도 웹 UI, 창, 타이머나 상주 파일 worker를 만들지 않았다. 실제 RAM 절감량은 측정하지 않았다.

검증 결과는 다음과 같다.

- Rust 단위 검사 231개 전부 통과. 이전 206개에 저장소 15개, 런타임 5개, 수집과 열기 경계 3개, 검색 2개를 추가했다. 실제 임시 파일의 재로드, 삭제, 만료, 개수와 2MiB, 쓰기와 동기화 실패, 외부 변경, pending, 권한과 링크, 대기와 삭제 순서, 오래된 세대, 종료, AI 소유권과 staging 상태를 검사했다. CEF 엔진과 AppKit을 초기화한 검사는 아니다. 기존 Unix 소켓 bind 검사를 위해 컴파일된 단위 검사 실행 파일만 승인된 권한으로 실행했다. `artifacts/history-v1-rust-tests.log`가 근거다.
- 기본 기능 전체 타깃 Cargo 검사와 browser 기능을 끈 CLI 구성 전체 타깃 검사, cargo fmt, git diff 공백 검사를 통과했다. 검사 전후 src 해시가 같다. 단위 구성은 observe_history, page_find_result, bookmark_completed의 dead_code 경고 3개, browser 기능을 끈 구성은 다운로드 권한 코드의 미사용 경고 8개가 남는다. 최초 컴파일의 반환 오류 타입 추론과 중첩 가변 borrow 두 지점을 수정한 뒤 다시 통과했다. 초기 실패는 `history-v1-tests-compile-initial.log`, 최종 결과는 `history-v1-tests-compile.log`, `history-v1-check.log`, `history-v1-cli-check.log`에 보존했다.
- 독립 읽기 리뷰에서 저장, 삭제 세대, staging과 소유권, UI 연결을 검토했다. 추가 필수 수정 사항은 남지 않았으나 실제 CEF callback 순서나 AppKit 동작의 증거는 아니다.
- 이번 변경에서 DOM 소스를 수정하지 않아 모의 DOM 검사를 새로 실행하지 않았다. 로컬 HTTP와 CEF 통합, 네이티브 화면과 IME, 외부 사이트, 장시간 기록과 RAM 비교는 실행하지 않았다. 반복 승인을 중단하라는 요청에 따라 앱 생성, 서명, 실행과 키체인 접근을 하지 않았다. 기존 바이너리의 과거 검증 결과를 이번 소스의 성공으로 합산하지 않았다.

`docs/history-design.md`에 정책, 호출 흐름과 후속 검증을 기록하고 README, 제품 계획, 아키텍처와 메모리 문서를 갱신했다. 같은 최종 앱에서 기록 수집, 검색과 전체 삭제, 재실행, AI 상태 유지와 화면 검증이 남는다. 전체 Naru 완성과 Preseller 내장은 계속 미완료다.

이번 검사 실행 세션은 모두 종료됐으며 새 앱, 보조 앱이나 서버는 시작하지 않았다. 기존 앱의 현재 프로세스 상태는 sandbox가 ps 실행을 거절해 재확인하지 않았다. 생성한 `target/.history-test-kx7wha93`의 파일 3,171개, 파일 크기 합계 2,025,839,863바이트, 약 1.89GiB를 제거했다. 물리 디스크 블록 회수량 측정은 아니다. 기존 앱과 CLI SHA-256은 작업 전후 같으며 사용자 프로필과 기존 탭 데이터를 건드리지 않았다. 종합 근거는 `artifacts/history-v1-validation.json`이다. 커밋, 푸시, 배포는 하지 않았다.

## 2026-10-15, 기존 탭 순환의 초안 보호와 메뉴 보완

독립 브라우저의 세로 탭과 키보드 사용성을 이어서 보완했다. 호출 흐름을 확인해 기존 Ctrl+Tab과 Cmd+Shift+괄호의 순환 기능이 이미 있음을 확인했다. 기존 경로는 목록 화면을 먼저 닫고 `cycle_tab → select → run_action`에서 실제 선택 검증 전에 주소 초안을 지웠다. 이번 변경은 순환 기능의 신규 구현이 아니라 이 선택 경로의 보호 보완이다. 실행 중 앱에서 초안 소실을 재현한 결과로 기록하지 않는다.

`src/control_panel.rs`는 번호 선택과 순환을 공통 `select_tab_command`로 연결한다. 활성 창과 입력기 조합을 확인한 뒤 최신 화면에서 대상을 계산하고 실제 공간 소속, 탭 수명 검사를 통과해야 선택한다. 실패 전에는 검색, 북마크, 다운로드 화면이나 초안을 지우지 않는다. 성공한 탭과 공간을 재확인한 뒤 목록을 닫고 최종 선택이 같은 경우만 포커스를 준다. 순환 키는 목록 처리와 본문 입력 관찰보다 먼저 소비하며 반복 키나 입력기 조합 중에는 선택하지 않는다. 번호 선택에도 같은 조합 보호를 적용했다. 본문의 일반 입력과 Cmd+대괄호의 뒤로, 앞으로 동작은 유지한다.

현재 공간의 사이드바 순서대로 양방향 순환하며 처음과 끝이 이어진다. 한 탭뿐이거나 현재 선택이 없으면 목록 화면에서도 아무 변화 없이 종료한다. 기존 번호 선택으로 같은 탭을 명시해 목록을 닫는 동작은 유지한다. 보기 메뉴에 이전 탭과 다음 탭을 추가하고 같은 경로로 실행한다. 순환마다 만들던 임시 Vec를 iterator로 교체했으며 새 캐시, 타이머, 저장소, 웹 UI는 추가하지 않았다. 메뉴 항목 두 개의 네이티브 자원은 추가되고 전체 RAM 절감량은 측정하지 않았다.

검증 범위는 다음과 같다.

- Rust 단위: 새 검사 3개를 포함한 206개 전부 통과. 다른 공간이 섞인 12개 탭의 순서와 양방향 끝 순환, 한 탭과 없는 선택의 무동작, 기존 단축키 별칭과 추가 수정키 거절을 검사했다. 메뉴 action의 편집 인계 대상 제외도 확인했다. 기존 실제 상태 기반 sidebar 선택 검사는 닫힘, adopt, release, rejected 탭 거절과 AI 소유권, revision 보존을 검사한다. 네이티브 창이나 CEF 엔진을 초기화한 검사는 아니다. 기존 Unix 소켓 bind 검사에 필요한 권한으로 컴파일된 단위 검사 실행 파일만 실행했다. `artifacts/tab-cycle-v1-rust-tests.log`가 근거다.
- 컴파일과 정적 검사: 단위 검사 컴파일, 전체 타깃 Cargo 검사, cargo fmt, git diff 공백 검사 통과. 전체 src 해시가 검사 전후 일치했다. 기존 검사 전용 dead_code 경고인 page_find_result, bookmark_completed 두 개는 남는다. `tab-cycle-v1-tests-compile.log`, `tab-cycle-v1-check.log`에 결과가 있다.
- 독립 읽기 리뷰: 단축키 우선 처리, 표시 공간 순서, 입력기 조회 뒤 대상 재계산, 검증 후 목록 닫기 경로를 확인했으며 추가 P1, P2 수정 사항은 없었다. 실제 키 이벤트, 반복 입력, IME, 오버레이 포커스와 실시간 경합의 재현 증거는 아니다.
- 모의 DOM, 로컬 CEF 통합, 네이티브 UI, 외부 사이트, RAM 비교는 실행하지 않았다. 반복 승인을 중단하라는 요청을 유지해 앱 번들 생성, 서명, 앱 실행과 키체인 접근을 하지 않았다. 최종 앱에 반영한 동작 검증은 남는다.

README와 제품 계획에 기존 순환 기능의 변경과 실행 한계를 기록했다. 아키텍처 문서에는 공통 선택 흐름을 추가하고, 본문 입력 감지가 아직 없다는 오래된 설명 두 곳을 실제 `send_event → observe_page_input → handoff_page_input` 흐름으로 바로잡았다. 본문 입력과 탭 선택의 제어권 차이를 문서에 명시했다.

검사 프로세스는 모두 종료됐으며 새 앱, 보조 프로세스나 서버를 시작하지 않았다. 이번에 만든 `target/.tab-cycle-test-g8cdc0qu`의 파일 3,070개, 파일 크기 합계 1,966,476,762바이트를 제거했다. 약 1.8GiB이며 물리 디스크 블록 회수량 측정은 아니다. 기존 앱과 CLI SHA-256은 작업 전후 같고 사용자 프로필과 기존 탭 데이터는 건드리지 않았다. 종합 근거는 `artifacts/tab-cycle-v1-validation.json`이며 변경 전 파일과 이번 국소 diff도 같은 접두사로 남겼다. 커밋, 푸시, 배포는 하지 않았다. 전체 Naru 완성과 Preseller 내장 목표는 계속 미완료다.

## 2026-10-15, 현재 주소를 새 사용자 탭에서 열기 소스 구현

Naru의 일상 탐색과 사용자, AI 공간 협업을 이어서 보완했다. 제품 계획을 다시 읽어 페이지 확대는 공유 host 확대값의 영향을 해결하지 못해 보류된 상태임을 확인했다. 해당 설계를 임의로 재개하지 않고 파일 메뉴에 `현재 주소를 내 공간 새 탭에서 열기`를 추가했다. 새 기능은 항상 새 사용자 탭을 만들며 원본 AI 작업의 소유권, revision과 원래 공간의 선택을 유지한다. 주소를 새로 탐색하는 기능이며 폼, 스크롤, 이력이나 문서 상태를 복제하지 않는다. 모든 공간의 기존 프로필 공유도 유지한다.

`src/control_panel.rs`는 실제 표시 탭의 대상 정보를 사용하고 주소창 초안은 읽지 않는다. 메뉴 validation과 실행에서 공간 목록, 각 목록 화면, 없는 뷰, 숨은 창과 페이지, 지원하지 않는 주소, 입력기 조합을 검사한다. 별도 단축키나 UI renderer를 추가하지 않았다. `src/browser.rs`는 실제 공간, 선택, 탐색 세대, 문서 준비와 소유권을 검사하고 CEF main frame의 HTTP(S) 주소를 읽는다. 생성 전후 실제 주소와 상태가 같은지 확인하고, 내 공간의 탭을 배경에 만든 뒤 연결과 검증이 성공한 경우에만 선택한다. 기존 `select_bookmark_tab`은 `select_user_tab`으로 이름을 일반화했으며 북마크와 최근 닫은 탭의 생성 정책은 바꾸지 않았다.

읽기 검토에서 CEF 호출 중 검색 화면이나 입력기 조합만 바뀌면 STATE 검사로는 잡지 못하는 경계를 확인했다. 실제 앱 재현으로 기록하지 않는다. 최종 경로는 CEF 조회 뒤 UI 상태를 다시 확인하고, 이어 원본과 소유권을 검사한 뒤 선택한다. 성공 후 주소 초안을 다시 지우던 중복 코드는 제거했다. 실제 선택 변경은 기존 Shell::render가 처리하며, 그 이후 새로 생긴 초안을 뒤늦게 삭제하지 않는다. 실패 뒤 정리는 이번에 만든 탭이 여전히 소유되지 않은 사용자 탭일 때만 수행한다. host 조회 뒤에도 자격을 재검사하고 rejected_creation으로 표시해 최근 닫은 탭 기록에 넣지 않는다. 다른 작업에 채택된 탭은 닫지 않는다.

검증 범위는 다음과 같다.

- Rust 단위: 새 검사 4개를 포함한 203개가 전부 통과했다. UI 대상과 화면 가드, 실제 주소 일치, 오래된 탭과 공간, epoch, 문서 상태, owner와 revision 변경, UI 변경 거절을 확인했다. 배경 삽입의 선택 보존, 정리 대상 자격과 실제 사용자 선택 함수의 A/B 작업 보존도 검사했다. inert CEF handle과 상태 검사이며 엔진 초기화나 화면 실행은 없다. 기존 Unix 소켓 검사에 필요한 bind를 위해 컴파일된 단위 검사 실행 파일만 샌드박스 제한 밖에서 실행했다. `artifacts/current-page-v1-tests-compile.log`, `current-page-v1-rust-tests.log`가 근거다.
- 컴파일과 정적 검사: cargo check 전체 타깃, cargo fmt 전체 검사, git diff 공백 검사 통과. `current-page-v1-check.log`에 결과가 있다. 검사 전후 전체 src 파일 해시가 같음을 확인했다. 기존 test 전용 dead_code 경고 2개는 남는다. 독립 최종 읽기 리뷰에서 추가 필수 결함은 없었다.
- 미실행: 모의 DOM 검사는 해당 소스를 변경하지 않아 반복하지 않았다. 앱 번들 생성, 서명, 실행, 로컬 CEF와 HTTP, 네이티브 메뉴와 포커스, 실제 입력기 조합, 생성 중 변경 경합과 생성물의 실제 종료, 외부 사이트, RAM 비교는 수행하지 않았다. 이전 46개 모의 DOM과 다른 앱 바이너리의 성공을 이번 네이티브 결과로 합산하지 않는다.

검사에는 별도 `target/.current-page-test-tjklle06`을 사용했다. 완료 후 일반 파일 3,070개, 파일 크기 합계 1,965,029,553바이트 약 1.8GiB와 해당 디렉터리를 제거했다. 이번에 실행한 단위 검사와 컴파일 프로세스는 종료됐고 앱, helper, 서버는 새로 띄우지 않았다. 보존 중인 앱 SHA-256은 `5eb253df67350236541fc613b4fcd9e6438492fcbf4f22ad4650c433c75b117b`, CLI는 `b0b57a113b48e321861675cef30d7cbb9e8a0aa12edd911c6aad108759625cbe`로 전후 일치한다. 프로필과 키체인은 접근하거나 변경하지 않았다. `artifacts/current-page-v1-validation.json`에 검사 결과, 미실행 범위와 정리를 기록했다.

`docs/open-current-page.md`, README, 아키텍처와 제품 계획을 갱신했다. 새 기능은 소스와 단위 검사 단계이며 보존 중인 numbered-tabs-v3 앱에는 아직 없다. 실제 앱 검증, 안정적인 서명과 최종 HTTP 검증, 방문 기록, 장시간 자원 회수, Preseller 내장은 남아 있다. Naru 전체 목표는 계속 진행 중이며 커밋, 푸시, 배포는 하지 않았다.

## 2026-10-15, AI 입력 종류 재검사와 완료된 DOM probe 해제

Naru 완성 후속으로 AI 제어의 입력·대기·취소 경로를 점검했다. 반복 키체인 입력을 요구하지 말라는 지시를 유지해 앱이나 CLI를 실행하지 않았다. `src/dom_bridge.js`의 fill은 지원 입력 종류를 검사한 뒤 focus를 주고 native value setter를 호출했지만, focus 처리 중 바뀐 종류는 재검사하지 않았다. text가 checkbox 등으로 바뀌어도 값을 쓰고 input/change 이벤트와 성공을 반환하는 경로를 모의 DOM에서 재현했다. 기존 정책을 `supportsFill`로 공유하고 focus 전과 쓰기 직전에 적용했다. 기존 거절 대상 file·checkbox·radio·button·submit·reset·image·hidden으로 바뀌면 fill의 setter와 이벤트를 실행하지 않는다. 지원하는 text↔password 전환은 유지한다. 페이지의 focus 처리 자체가 만든 변경을 취소하거나 실제 파일 입력 동작을 재현한 검사는 아니다.

`src/browser.rs::devtools_result`의 ReadyState 최종 응답도 수정했다. pending을 제거한 뒤 탐색 오류나 기한 만료로 즉시 반환하면 waiter의 `probe_in_flight`가 true로 남았다. 다음 타이머 전에 새 탐색이 오류를 지우는 상태 순서에서는 새 문서 확인을 막을 수 있다. 해당 최종 분기에서 기존 `pending_error`의 ReadyState 경로를 호출해 표시만 해제한다. waiter와 응답은 살아 있고 기존 타이머가 오류·시간 초과 및 탭 상태를 반환한다. Frame/World 준비 단계, 변경 작업과 캡처 처리에는 손대지 않았으며 자동 행동 재시도를 추가하지 않았다. 기존 inert CEF handle과 실제 STATE·devtools_result를 쓰는 회귀에서 오류·기한 만료, pending 제거, probe 해제, 즉시 성공 없음, 중복 콜백 무효화를 검사한다.

검증은 다음과 같이 구분한다.

- 모의 DOM: 기존 37개 통과를 확인한 뒤 새 회귀를 넣었고 수정 전 46개 중 38개 통과·8개 실패, 수정 후 46개 전부 통과했다. 부모도 최종 소스로 46개를 다시 실행해 통과했다. `artifacts/dom-fill-focus-red.log`, `dom-fill-focus-green.log`, `ai-guards-v1-dom-tests.log`가 근거다.
- Rust 단위: 새 회귀만 먼저 실행해 수정 전 probe 표시 assertion 실패(exit 101)를 확인했다. 수정 후 전체 검사 최초 실행은 196개 통과·3개 실패였다. 실패 3개 모두 자체 Unix 소켓 bind의 샌드박스 제한이었다. 컴파일된 동일 단위 검사 실행 파일만 제한 밖에서 다시 실행해 199개 전부 통과했다. `ai-guards-v1-rust-before.log`, `ai-guards-v1-rust-tests.log`, `ai-guards-v1-rust-tests-unrestricted.log`에 실패와 성공을 구분했다. 기존 test 전용 dead_code 경고 2개는 남는다.
- 정적 확인: cargo fmt 전체 검사와 git diff 공백 검사 통과. 독립 읽기 리뷰에서 필수 수정 사항은 없었다.
- 미실행: 앱 번들 생성·서명·실행, 로컬 HTTP/CEF 통합, 네이티브 UI, 외부 사이트, 실제 Chromium focus 처리 및 타이머와 탐색의 경합, RAM 비교는 하지 않았다. 단위 검사 실행을 실제 CEF 브라우저 실행으로 세지 않는다. 전체 타깃 검사와 배포 빌드도 이번 범위에 없다.

검사 컴파일은 별도 `target/.ai-guards-test-ih0t4yo3`에서 수행하고 완료 후 해당 디렉터리를 제거했다. 제거한 일반 파일 2,791개의 파일 크기 합계는 1,511,134,272바이트(약 1.4GiB)다. 새 앱·helper·서버 프로세스를 띄우지 않았고 검사 실행 파일은 모두 종료됐다. 기존 앱 SHA-256 `5eb253df67350236541fc613b4fcd9e6438492fcbf4f22ad4650c433c75b117b`와 CLI `b0b57a113b48e321861675cef30d7cbb9e8a0aa12edd911c6aad108759625cbe`가 전후 일치하며 프로필·키체인을 변경하지 않았다. `artifacts/ai-guards-v1-validation.json`에 소스 해시, 실제 검사 결과와 정리를 남겼다. 다른 앱의 현재 실행 여부를 조사한 결과는 아니다.

`docs/architecture.md`와 `docs/product-plan.md`에 동작과 증거 범위를 반영했다. 수정 소스는 아직 보존 중인 numbered-tabs-v3 앱에 번들링하지 않았다. 변경 빌드 간 서명 정체성과 키체인 권한 유지, 최종 HTTP/CEF 검증, 장시간 자원 회수와 실제 메모리 비교, Preseller 내장은 남아 있다. 이번 단위·모의 검사를 이 범위나 Naru 전체의 완료로 표시하지 않는다. 커밋·푸시·배포는 하지 않았다.

## 2026-10-15, 개발 빌드와 앱 실행 분리

Naru 완성 작업을 이어가되 반복 키체인 입력을 요구하지 말라는 사용자 지시를 유지했다. 지난 턴의 숫자 탭 선택과 초안 보호는 구현 및 네이티브 검사에서 진전이 있었지만, 최종 HTTP 검사는 미완료다. 이번에는 앱이나 키체인을 조작하지 않고 `scripts/dev.sh`의 기본 자동 실행 경로를 수정했다. 기존 run_app=1은 빌드와 번들 교체가 끝나면 프로필을 사용하는 앱을 즉시 실행했다. 현재 번들의 공개 서명 메타데이터도 Signature=adhoc, TeamIdentifier=not set, cdhash 기반 designated requirement였다. [Apple TN3127](https://developer.apple.com/documentation/technotes/tn3127-inside-code-signing-requirements)은 ad hoc의 designated requirement가 해당 코드 버전에 묶인다고 설명한다. 이 자료를 인증서나 키체인 항목을 조회한 증거로 쓰지 않는다.

기본값은 빌드만 수행하도록 바꿨다. 앱 실행은 명시적 `--run`에만 남겼고, 서명자가 미지정, 빈 값, ad hoc 값 `-`이면 다운로드나 컴파일 전에 거절한다. `--run`과 `--build-only`의 충돌도 거절한다. 빌드 전용 모드에서는 `--` 뒤의 브라우저 인수를 무시하지 않고 오류로 처리한다. identity와 브라우저 인수는 기존 배열 및 개별 인수 전달을 유지한다. 번들 생성, 실패 시 중단, strict 검증 후 이전 앱 교체, 중간물 정리는 바꾸지 않았다. 빌드 전용 모드도 실제로 실행하면 기존 앱 교체 과정에서 이전 앱을 종료할 수 있음을 README에 유지했다. README, `docs/architecture.md`, `docs/product-plan.md`에 변경과 미완료 범위를 반영했다.

`tests/dev_script_test.py`는 실제 개발 스크립트를 공백이 있는 임시 checkout에 복사하고 PATH를 가짜 도구만 있는 디렉터리로 제한한다. fetch, Cargo, Python 단계, 앱은 호출 인수만 기록하며 실제 명령으로 전달하지 않는다. 기본 debug와 release의 앱 비실행, 명시적 빌드 전용과 identity 전달, 실행 옵션의 인수 보존, 잘못된 identity와 옵션의 조기 거절, 각 단계 및 Ninja 다운로드 실패 뒤 중단을 검사했다. `python3 -B -m unittest discover -s tests -p dev_script_test.py -v`의 9개 검사가 통과했고 Bash 문법 검사와 git diff 공백 검사도 통과했다. 부모가 별도로 재실행한 결과는 `artifacts/dev-launch-policy-v1-tests.log`에 있다. 독립 읽기 리뷰에서 필수 수정 사항은 없었다.

실제 checkout의 `scripts/dev.sh --release --run`도 NARU_CODESIGN_IDENTITY를 제거한 환경에서 실행해 예상한 exit 2와 빈 stdout을 확인했다. 거절은 fetch나 Cargo 이전이며 앱을 실행하지 않았다. `artifacts/dev-launch-policy-v1-preflight.json`에 결과와 기존 앱, CLI 해시 일치를 남겼다. 앱 SHA-256은 `5eb253df67350236541fc613b4fcd9e6438492fcbf4f22ad4650c433c75b117b`, CLI는 `b0b57a113b48e321861675cef30d7cbb9e8a0aa12edd911c6aad108759625cbe`로 이전 최종 바이너리와 같다. 이번 작업은 실제 서명, Rust 재빌드, 앱 실행, 모의 DOM, 로컬 CEF, 네이티브 UI, 외부 사이트, RAM 비교를 수행하지 않았다. 이전 Rust 198개 결과를 이번 검사 수에 합산하지 않는다.

임시 checkout과 가짜 앱은 검사 종료 시 제거됐다. 새 앱 번들이나 CEF 프로세스는 만들지 않았으며 프로필, 키체인, 기존 바이너리는 그대로다. 지속적인 서명 정체성과 변경 빌드 사이의 권한 유지 검증, 최종 HTTP 및 CEF 검증은 아직 남아 있다. 이번 변경을 반복 승인 문제나 독립 브라우저 전체의 해결로 표시하지 않는다. 전체 목표는 계속 진행 중이며 커밋, 푸시, 배포는 하지 않았다.

## 2026-10-13, 공간별 숫자 탭 단축키와 주소 초안 보완 — HTTP 검증 미완료

일상 탐색 후속으로 `src/control_panel.rs`에 ⌘1–8은 현재 표시 공간의 사이드바 순서, ⌘9는 마지막 탭을 고르는 동작을 추가했다. 없는 번호와 반복 키는 페이지에 넘기지 않으며, 오버레이·공간 보기 밖에서 같은 탭을 다시 선택하면 입력을 건드리지 않는다. `src/browser.rs::select_tab_from_sidebar`가 실제 공간·소속·살아 있는 탭을 먼저 검사하고 기존 AI 제어권을 유지하는 선택 경로를 재사용한다. 성공 후에만 목록을 닫고 최종 포커스 대상도 재확인한다. 새 저장소·renderer·타이머·탭 목록 캐시는 없다.

중간 v1은 Rust 197개·전체 타깃·format·release를 통과했지만 실행하지 않았다. 읽기 리뷰에서 낡은 UI 탭 정보로 선택에 실패하기 전에 오버레이를 닫을 수 있는 순서를 발견해 v2에서 수정했다. v2는 Rust 198개·전체 타깃·format·release가 통과했다. 실제 Cmd1/2/9 선택, 없는 Cmd8에서 주소·검색·북마크 검색 입력과 포커스 유지, 검색에서 유효한 숫자 선택, 사용자·AI 공간별 순서와 A/B active revision 1 보존을 확인했다. 로컬 HTTP는 초기 로딩 대기 후 같은 바이너리에서 완료됐으며 commit·문서 준비·메인 로드와 실제 화면·소유 작업 범위의 read가 일치했다. 추가 키체인 요청은 하지 않았고 해소 원인을 단정하지 않는다. 작업 소유 탭의 비범위 read는 TASK_REQUIRED로 거절됐으며 소유 범위로 재검사했다. 근거는 `artifacts/numbered-tabs-v2-native.json`, `-ai-http.json`이다.

v2 네이티브 검사에서는 상단 주소 초안→북마크→현재 탭 번호 선택에서 초안 소실을 발견했다. 숫자 선택 없이 북마크→Esc만으로도 같은 현상이 재현됐다. 기존 중앙 시작 입력 보호와 별개로, 상단 주소 보호가 다운로드 진입에만 있었다. v3는 기존 metadata-only AddressDraft 보관을 세 오버레이 진입에서 공통 사용하고, 이미 열린 오버레이·공간 보기에서는 숨은 입력을 새 초안으로 채택하지 않도록 했다. 실제 탭·공간·URL·탐색 세대 변경 시 만료 규칙은 유지했다. 최종 v3는 Rust 198개·전체 타깃·format·release가 통과했고 소스 해시가 빌드 전후 일치했다. 앱 SHA-256은 `5eb253df67350236541fc613b4fcd9e6438492fcbf4f22ad4650c433c75b117b`이며 CLI는 `b0b57a113b48e321861675cef30d7cbb9e8a0aa12edd911c6aad108759625cbe`다. 기존 test 전용 dead_code 경고 2개는 유지된다. 최종 앱에서 상단 주소→북마크→Esc, 검색·북마크·다운로드→현재 번호로 복귀할 때 초안을 보존했다. 복귀 시 주소 전체 선택을 확인했고 Right 뒤 붙여넣기로 이어쓰기를 확인했다. 없는 Cmd8의 검색·북마크 검색·다운로드 포커스 보존, 같은 탭 Cmd9의 입력 보존, 다른 Cmd1 선택의 이전 초안 초기화, 중앙 시작 입력의 기존 보호, 공간 보기에서 Cmd2 복귀, 배경 작업 시작의 사용자 선택 보존과 사용자 Cmd9의 공간 내 마지막 선택도 확인했다. AI 공간의 Cmd1/2/9·없는 Cmd8·검색→Cmd1은 A/B 모두 active revision 1을 유지했다. 숫자 3–8의 실제 각 위치·키 반복·IME·숫자 패드·실시간 stale UI 경합은 네이티브 검사 범위가 아니다. 근거는 `artifacts/numbered-tabs-v3-native.json`과 `-before-approval.json`이다.

최종 v3의 첫 HTTP load는 5,021ms에 WAIT_TIMEOUT이었고, Naru 자체 스택에서 SecKeychainItemCopyContent→CSSM_DecryptDataFinal 응답 대기를 확인했다. `numbered-tabs-v3-first-http.json`과 `-http-stack.txt`에 기록했다. Computer Use의 SecurityAgent 접근 차단 때문에 사용자에게 직접 승인을 요청했다. 사용자는 반복 입력·승인 요청을 중단하라고 했으며 이를 새 승인으로 해석하지 않았다. 추가 승인 요청과 재빌드를 멈췄고 HTTP·CEF 16개는 미검증으로 남겼다. 이전 v2 HTTP 성공이나 다른 빌드의 회귀를 v3에 합산하지 않는다.

v2 검증용 A/B 작업·사용자 탭 2/3을 정리하고 원래 사용자 빈 탭 1을 보존했다. 네이티브 CmdQ는 exit 0이었다. 최종 v3도 빌드 중간물 4,332개·2.5GiB를 제거했으며 최신 dist 번들 하나 약 335MiB, target 약 2.9MiB, 검증 자료 약 20.2MiB, 재사용 CEF 캐시 약 339MiB를 유지한다. 최종 v3의 A/B 작업은 모두 finished·빈 tabs/closingTabs로 정리했고 검사 탭 2–6을 닫아 원래 사용자 빈 탭 1과 빈 입력만 남겼다. pendingWaits·capture·탭 생성·다운로드 대기는 0이었다. 네이티브 CmdQ로 v3 PID 86179를 정상 종료해 exit 0을 확인했고 재시작하지 않았다. 자체 HTTP fixture PID 69901(session 60273, port 61209)은 SIGTERM 뒤 exit 0으로 종료했다. v1은 미실행이며 이전 앱과 최종 검증 앱도 남아 있지 않다. 프로필과 기존 데이터는 보존했다. 반복 승인을 요구하는 개발 서명 문제는 해결되지 않았으며 사용자가 거절한 보안 승인을 우회하거나 제공된 암호를 사용하지 않았다. README와 제품 계획에 완료된 검사와 승인 대기를 구분했으며 새 모의 DOM·외부 사이트·RAM 비교·전체 제품 완성은 주장하지 않는다. 커밋·푸시·배포는 하지 않았다.

## 2026-10-13, 사이드바 재현 조건 분리와 제한된 진단 추가

UI 안정화 후속으로 사이드바 잘림의 재현 조건을 확인했다. 기존 `native-focus-v3` 실행에서 접기·펼치기 두 차례, 같은 바이너리 재시작 후 한 차례 모두 정상 모서리였다. 독립 CUA 관찰도 왼쪽 24px 여백과 둥근 모서리를 확인했다. 이전 초기 검사에서의 잘림은 취소하지 않으며, 재시작만으로 재현되는 현상이라고 단정하지 않는다. 기존 Naru 표본 두 개에서 키체인 복호화 대기는 별도 Chromium worker였고 main은 AppKit 이벤트 대기였다. 메인 스레드의 중첩 루프가 잘림의 원인이라는 근거는 없었다. `artifacts/surface-probe-v1-baseline.json`과 기존 `ui-focus-v{2,3}-first-http-stack.txt`에 범위를 구분했다.

`src/control_panel.rs`에 `NARU_SURFACE_PROBE` 선택형 진단을 추가했다. 미설정·오타는 즉시 비활성화하며 `noop`, `frame`, `bounds`, `visible`, `convert`, `layer`, `presentation`만 받는다. 기존 포커스 또는 전체 표면 진단과 동시에 켜면 실행하지 않는다. 사이드바 전환의 layout 뒤 최대 16회 시도하며, 상태 borrow 밖에서 기존 두 배경의 선택한 getter만 조회한다. `presentation`은 `layer` 조회가 선행된다. noop은 같은 retain·카운터·고정 로그를 거치되 추가 getter는 호출하지 않는다. 입력값·주소·페이지·포인터를 출력하지 않고 새 타이머·이미지 캐시·레이어·웹 renderer는 만들지 않는다. 기본 표시 동작의 수정이나 잘림 해결로 설명하지 않는다.

처음 format 검사는 import 정렬 차이로 실패했고 해당 로그를 보존했다. 프로젝트 `cargo fmt` 적용 뒤 format·release 빌드는 성공했으며 빌드 전후 28개 소스 해시 일치를 확인했다. 앱 SHA-256은 `0a83c27b979736846c67070c98a877c95ad6817439eab182e8dc5e446d76362e`, CLI는 `1de0078a544b28c5d0e2acf9dca685bea48ccec934accaa2693304a74dc4a5c3`다. 진단 추가에 대한 빌드·네이티브 확인으로 범위를 잡았으므로 Rust 단위·전체 타깃·모의 DOM·CEF 32개 전체 회귀는 다시 실행하지 않았다. 이전 v3의 194개·32개 결과를 이 바이너리에 합산하지 않는다.

새 바이너리의 진단 OFF 첫 실행에서 접기·펼치기 후 화면은 정상이었고 진단 로그는 0줄이었다. 종료는 teardown watchdog에 의해 exit 2였으며 정상 종료로 세지 않는다. 종료를 기다리는 중 다음 실행 명령을 제출했지만 이전 세션의 exit 2와 후속 단일 메인 PID 67567을 확인했다. 이후 재시작은 이전 세션 종료 확인 뒤 진행했다. noop 실행에서는 실제 UI를 18회 전환해 AX의 접힘·펼침 상태를 확인했고 로그가 정확히 sequence 1–16에서 멈췄다. 마지막 화면은 정상 모서리였다. 매 전환의 이미지를 판독한 것은 아니다. 재현 대조군이 없으므로 getter별 모드는 실행하지 않았고 어느 조회가 현상을 고친다는 판정도 하지 않았다. noop 실행의 자체 Naru 스택 표본에는 키체인 복호화 호출이 없었으며 종료는 exit 0이었다. 최초 종료 실패의 원인은 미확정이다.

같은 바이너리를 다시 기본 설정으로 실행해 원래 사용자 탭 1을 보존한 검증 탭 2에서 native CmdT, 주소창 Tab→중앙 입력, 사이드바 북마크 왕복 후 초안 보존을 확인했다. 중앙 입력 Enter로 자체 HTTP fixture를 열었고 실제 화면·CLI read·commit·문서 준비·메인 로드 완료가 일치했다. 추가 키체인 질문 없이 완료했으나 변경 빌드 사이 권한 유지의 해결로 확대하지 않는다. 검증 탭은 CmdW로 닫았고 이 기본 실행도 exit 0이었다. 결과는 `surface-probe-v1-native.json`, `-http.json`, `-noop-app.log`, `-final-app.log`에 기록했다. 실제 IME·FKA 활성·최소 창·다크 모드·외부 사이트·비교 RAM은 미실행이다.

기존 앱과 모든 중간 실행은 종료했고 최신 기본 실행 메인 PID 68075 하나와 CEF helper만 남겼다. 자체 fixture PID 67440은 SIGTERM 후 exit 0으로 종료했다. 원래 user 빈 탭 1과 빈 입력을 보존했으며 작업 목록·wait·capture·탭 생성·다운로드·권한·저장 대기는 모두 0이다. 빌드 중간물 1,263개·672.1MiB를 제거했고 최신 dist 번들 하나 약 335MiB, target 약 2.9MiB, 검증 자료 약 19.6MiB, CEF 캐시 약 339MiB를 유지한다. 최종 재시작 뒤 HTTP는 반복하지 않았다. 정리 및 소스·로그 검사는 `surface-probe-v1-final-state.json`, `-validation.json`에 남겼다.

README·제품 계획·시작 화면 설계에 초기 실패와 후속 비재현을 구분해 반영했다. 이번 완료 범위는 제한된 진단 도구와 기본 입력·탐색 확인이다. 사이드바 원인, 최초 종료 지연, 나머지 독립 브라우저 기능과 Preseller 내장은 남아 있으며 전체 목표는 활성 상태다. 커밋·푸시·배포는 하지 않았다.

## 2026-10-13, 숨긴 Chromium 키보드 포커스 보정과 사이드바 갱신 진단

이전 키체인 승인 뒤 이어진 UI 보완으로 주소창과 새 탭 중앙 입력의 Tab 이동을 수정했다. `src/mac.rs`의 기존 `super.sendEvent`가 끝난 뒤, 같은 Naru key window의 일반 Tab·ShiftTab에서 실제 first responder가 숨긴 CEF 하위 뷰인 경우에만 보정한다. `src/control_panel.rs`는 AppKit의 다음·이전 유효 후보 순서와 `canBecomeKeyView` 조건을 유지하면서 숨긴 후보를 건너뛴다. 포커스를 바꾸기 직전 창·원래 responder·후보를 다시 검사하며 모든 AppKit 호출은 상태 borrow 밖에서 실행한다. 순환과 512개 후보 상한을 두었고 정상 웹 입력·마우스·직접 포커스·task/space 상태 API는 바꾸지 않는다. 방문 목록은 예외 경로에서만 생기며 arm64에서 최대 512개 참조다. 새 UI renderer·타이머·저장소는 없다.

`native-focus-v1` 실제 진단에서 주소창의 nextValidKeyView와 중앙 입력의 previousValidKeyView가 hidden·hiddenAncestor=true인데도 key=true인 `RenderWidgetHostViewCocoa`였다. 일반 Tab에서 OnSetFocus 없이 OnGotFocus가 호출됐고, 이벤트 종료 뒤에도 해당 숨긴 뷰가 responder였다. 이후 기존 시작 화면 fallback이 주소창으로 돌렸다. 이를 단순 키 목록 갱신으로 해결됐다고 설명하지 않는다. v2는 진단 ON에서 주소 Tab→중앙·중앙 ShiftTab→주소 2방향, OFF에서는 네 방향을 확인했다. `src/browser.rs`의 포커스 콜백과 `src/mac.rs`, `src/control_panel.rs`의 선택적 진단은 입력값·URL·제목·포인터 주소를 기록하지 않는다. `NARU_FOCUS_DIAGNOSTICS=1`은 최대 128줄, `NARU_SURFACE_DIAGNOSTICS=1`은 첫 토글 이후 최대 32샘플이며 기본은 꺼져 있다. 후보 조회가 AppKit의 지연 계산 시점을 바꿀 수 있다는 한계도 남긴다.

사이드바 문제는 아직 미해결이다. v1의 검색·새 탭 배경 frame 변경 시 setNeedsDisplay 후보는 펼친 뒤 무입력 후속 캡처에서도 왼쪽 잘림이 남았으며 실제 좌표 클릭으로도 재현됐다. CmdL 뒤에는 정상으로 돌아왔다. v2 수치 진단은 view/model의 x=12pt·정상 bounds를 보였고, 토글 직후 presentation만 이전 폭이었다. 같은 바이너리에서 전체 진단 ON은 정상, OFF는 잘림이 재현돼 진단 읽기가 현상을 가릴 가능성을 확인했다. 특정 getter나 음수 좌표가 원인이라고 확정하지 않는다. v3는 모든 배치와 상태 borrow 종료 후 frame이 바뀐 두 배경에만 공개 `displayIfNeeded()`를 호출했지만 진단 OFF 펼침 직후 잘림이 다시 나타났다. 이 후보를 해결로 표시하지 않으며 개별 getter와 무동작 I/O 대조는 후속 계획일 뿐 실행하지 않았다.

v1은 format·release 빌드만 실행했다. v2와 최종 v3는 각각 Rust 단위 검사 194개, 전체 타깃 Cargo 검사, format·release 빌드를 통과했다. 새 모의 DOM 검사는 실행하지 않았다. 기존 test-only dead_code 경고 2개는 남는다. 최종 앱 SHA-256은 `03e369b4c43b551e473071658d27d0902a1e6d0f67202ac4514327ad6511d226`, CLI는 `b93656475cc0b2a17ffec2c29950a5f8f36ebfb9188ccec03a7965b55c687d7e`다. 빌드 전후 소스 해시가 같음을 확인했다. `artifacts/native-focus-v3-{source,binaries}.json`과 `-rust-tests.log`, `-check.log`, `-fmt.log`, `-build.log`에 근거를 남겼다.

최종 v3는 두 진단을 끈 상태에서 검증했다. 회귀 검사가 끝난 뒤에도 주소 Tab→중앙·중앙 Tab→주소·주소 ShiftTab→중앙·중앙 ShiftTab→주소 네 방향을 실제 AX 포커스로 확인했다. Tab으로 중앙에 진입해 입력하고 사이드바 북마크·검색·다운로드를 왕복해 현재 초안을 보존했다. 선택된 입력에 End를 보낸 뒤 붙여넣으면 교체됐으므로 이어쓰기 성공으로 세지 않았고, Right로 선택을 해제한 대조에서는 정상으로 이어 썼다. AI 탭 채택 후에도 사용자 공간이 유지되는 것을 늦게 확인해 처음 의도한 AI 검사는 사용자 탭에서 실행됐다. 원래 탭에 넣은 검증 문자열을 지우고 실제 A 공간으로 전환해 다시 검증했으며, 마지막 원래 탭의 중앙 입력이 빈 placeholder인 것을 확인했다.

실제 A 공간에서 Tab 이동·입력은 A/B 모두 active revision 1을 유지했다. B의 배경 HTTP 탭 생성·완료 후에도 A의 선택과 편집기가 유지돼 재포커스 없이 입력했고, 유효한 중앙 HTTP Enter만 A를 paused revision 2로 바꾸고 B는 active revision 1로 유지했다. 이번 최종 바이너리의 HTTP는 추가 키체인 질문 없이 완료됐다. 향후 다른 바이너리에서도 승인 대기가 해결됐다는 뜻은 아니다. 실제 웹 입력에서 Tab으로 팝업 버튼에 이동해 Return으로 내용이 있는 about:blank 팝업을 만들었으며, 팝업의 Tab·붙여넣기 값은 CLI read와 일치했다. 근거는 `native-focus-v3-native.json`, `-ai-keyboard.json`, `-handoff.json`, `-http-wait.json`, `-popup-read-after.json`이다. 한글은 붙여넣기 검사이며 IME 조합·FKA 활성 설정·최소 창·다크 모드·외부 사이트·장시간·비교 RAM은 실행하지 않았다.

동일 최종 바이너리의 로컬 CEF select 10개·tasks 6개·spaces 10개·single_window 6개, 총 32개가 exit 0·빈 stderr·cleanupVerified true로 통과했다. 첫 실행기는 리다이렉션 로그 이름이 자체 중복 방지 규칙에 걸려 앱 접근 전에 종료됐다. 해당 로그를 보존해 다른 이름으로 옮긴 뒤 처음으로 검사들을 실행했으며 제품 실패를 재시도한 것으로 합산하지 않는다. 마지막 자료 검사의 첫 assertion은 stderr 파일 경로를 내용으로 잘못 해석해 실패했다. 실제 스키마의 stderrBytes와 해당 파일 크기를 검사해 모두 0임을 확인했으며 앱 검사는 반복하지 않았다. `native-focus-v3-harness-preflight-error.log`, `-suite.json`, `-suite-state.json`, `-regression-run.log`로 구분한다.

이전 앱은 각 재빌드 전에 CmdQ로 종료했으며 중간 진단 앱과 진단 OFF 대조 실행도 종료했다. 최종 메인 PID 61776과 그 CEF helper만 유지한다. 자체 서버 PID 60445는 SIGTERM 후 exit 0으로 종료했다. v1 빌드는 중간물 1,263개·671.9MiB, v2·v3는 각각 4,332개·2.5GiB를 정리했다. target 2.9MiB·최신 dist 번들 하나 335MiB·검증 자료 약 19.3MiB·CEF 캐시 339MiB를 유지한다. 사용자 프로필과 원래 탭 1을 보존했고, 네이티브 2개와 회귀 9개 작업 모두 finished·소속/종료 대기 탭 없음으로 정리했다. 원래 user 탭 1만 선택됐고 다운로드·저장·권한·wait·capture·탭 생성 대기는 0이다. `native-focus-v3-native-cleanup.json`, `-final-state.json`, `-validation.json`에 결과를 기록했다.

README·제품 계획·시작 화면 설계를 갱신했다. 이번 완료 범위는 키보드 이동 보정과 그 회귀 검증이며, 사이드바 잘림·나머지 제품 범위·Preseller 내장은 완료가 아니다. 전체 목표는 활성 상태다. 커밋·푸시·배포는 하지 않았다.


## 2026-10-13, 입력 초안 보존 보완과 키보드·사이드바 검증

새 탭 화면의 후속 네이티브 검증을 진행했다. `src/control_panel.rs`에서 검색·새 탭의 장식 배경 두 개에 `clipsToBounds`를 명시하고, 최초 배치와 실제 표시 전환 뒤 AppKit 키 뷰 목록을 갱신했다. 일반 타이핑·주기 갱신에는 목록을 다시 계산하지 않는다. 중앙 입력에서 주소창으로 Tab 이동은 확인했지만 반대 방향의 Tab·ShiftTab은 여전히 주소창에 머물렀다. 키보드 순환 해결로 표시하지 않으며 숨긴 Chromium 자식의 키 뷰 참여 여부는 후속 런타임 진단 항목이다.

중간 `ui-focus-v2`에서 중앙 필드를 편집한 채 **사이드바** 북마크를 열고 Esc로 돌아오면 같은 탭 3·epoch 2의 초안이 없어졌다(2회). Tab으로 편집을 먼저 종료한 대조군은 값을 보존했다. 이전 시작 화면 검증의 중앙 북마크 버튼 경로와 구분한다. `ui-focus-v3`는 `sync`의 사전 편집 종료 조건을 대상 변경뿐 아니라 시작 화면이 곧 숨겨지는 경우로 확장했다. 기존 field editor 종료를 상태 borrow 밖, `setEnabled(false)`보다 먼저 수행한다. 입력 문자열을 별도 저장하거나 다시 쓰지 않으며 새 대상 초기화·AI 제어권 검증은 유지한다.

v1·v2·v3는 각각 Rust 단위 검사 194개, 전체 타깃 Cargo 검사, format 검사와 release 빌드를 통과했다. 기존 테스트 전용 dead_code 경고 2개는 남는다. 새 모의 DOM 검사는 실행하지 않았다. 최종 앱 SHA-256은 `eacac76ec88bf8744db9f73d3191bc00fbe0dd15a3a18719ee8f755aa5d59da5`, CLI는 `47070c0aca674370d4e4af0d98e957396b5816ff4eeb31cabfff4b9cdc991450`이다. 바이너리·소스 해시 및 검사 로그는 `artifacts/ui-focus-v3-binaries.json`, `-source.json`, `-rust-tests.log`, `-check.log`, `-fmt.log`, `-build.log`에 있다.

v3 실제 화면에서 편집 중 사이드바 북마크·통합 검색 단축키·다운로드 왕복의 초안 보존을 확인했다. 접기·펼치기와 배경 AI 탭 생성 뒤에도 사용자 초안에 재포커스 없이 이어 썼다. AI 작업 A의 중앙 입력·북마크 왕복은 A/B 모두 active revision 1을 유지했고, 유효 주소 Enter 제출은 A만 paused revision 2·B active revision 1로 바꿨다. 창 크기 변경 시도는 2360×1664픽셀을 바꾸지 못해 최종 최소 창 검사는 미검증이다. 접힌 검색·새 탭 배경은 정상적으로 보였고 펼친 직후 캡처에는 왼쪽 배경이 잘렸으나 후속 Tab 화면은 정상 모서리였다. 표시 전환 직후의 시각 안정성까지 해결했다고 기록하지 않는다. 한글은 붙여넣기이며 IME 조합·다크 모드·전체 버튼 키보드 이동·외부 사이트·메모리 비교는 미검증이다.

v2 첫 HTTP는 잘못된 wait 옵션을 준 하네스의 INVALID_ARGUMENTS 뒤, 올바른 호출에서도 5,042ms WAIT_TIMEOUT이었다. 제품 대기와 하네스 오류를 구분한다. 앱 표본 831개가 `SecItemCopyMatching → SecKeychainItemCopyContent → decrypt`에서 대기했고, 사용자 “항상허용했다” 응답 후 같은 바이너리에서 HTTP 완료를 확인했다. 실제 about:blank 팝업 본문과 입력을 CLI read로 대조했다. v2 전체 CEF 32개는 실행하지 않았다. v3도 첫 HTTP가 문서 전 대기하며 같은 키체인 호출 스택 807개를 확인해 사용자에게 직접 승인을 요청했다. SecurityAgent 도구 접근 차단을 우회하거나 인증정보를 사용하지 않았다. 변경 빌드 간 권한 유지 문제를 해결했다고 설명하지 않는다.

사용자 “했다” 응답 후 추가 재빌드 없이 v3의 AI A/B HTTP 완료를 확인했다. 사용자 탭 3의 선택과 중앙 초안은 배경 생성·완료 후 유지됐고 이어 썼다. 사용자 중앙 Enter로 자체 HTTP를 열고 내용이 있는 about:blank 팝업을 실제 클릭으로 생성했다. 팝업 본문이 보였으며 붙여넣은 값은 CLI read와 일치했다. B 작업의 CLI read 후에도 A paused revision 2·B active revision 1이었다. 최종 같은 바이너리의 select 10개·tasks 6개·spaces 10개·single_window 6개, 총 32개 로컬 CEF 회귀가 모두 exit 0·빈 stderr로 통과하고 cleanupVerified도 true였다. 근거는 `artifacts/ui-focus-v3-popup-read.json`, `-user-http.json`, `-ai-read.json`, `-suite.json`, `-suite-state.json`이며 이전 빌드 결과를 합산하지 않았다.

이전 앱 PID 19977·31866·37602는 CmdQ로 정상 종료(exit 0)했고 v3 메인 PID 44339 하나가 실행 중이다. 각 빌드는 Cargo 중간물 4,332개·2.5GiB를 정리했다. 자체 서버 PID 31756도 SIGTERM 후 exit 0으로 종료했다. 최신 번들 하나, target 2.9MiB·dist 335MiB·검증 자료 18.8MiB·CEF 캐시 339MiB를 유지한다. 사용자 프로필과 원래 탭 1은 보존했으며 검증용 탭을 닫고 자체 작업 11개 모두 finished·소속/종료 대기 탭 없음으로 정리했다. 원래 사용자 탭 1만 선택됐고 다운로드·권한·저장·wait·capture·탭 생성 대기는 모두 0이다. `artifacts/ui-focus-v3-native-cleanup.json`, `-final-state.json`, `-validation.json`에 최종 상태를 남겼다. 중간 실패와 최종 결과는 `ui-focus-v1-native.json`, `ui-focus-v2-native.json`, `ui-focus-v3-native.json`에 구분했다.

README·제품 계획·시작 화면 설계를 갱신했다. 전체 목표는 활성 상태이며 Tab 역방향 경로, 사이드바 전환 표시, 변경 빌드 사이 키체인 권한 유지와 나머지 제품 범위를 완료로 표시하지 않는다. 커밋·푸시·배포는 하지 않았다.

## 2026-10-13, 네이티브 새 탭 시작 화면과 제출 포커스 수정

Aside 형식에 토스의 시각 표현을 참고하는 후속으로, 빈 새 탭에 중앙 주소·Google 검색 입력과 열기·북마크 진입을 구현했다. `src/control_panel.rs`, `src/browser.rs`, `src/start_page.rs`, `src/lib.rs`를 연결했다. 창당 고정 AppKit 화면을 재사용하며 별도 창·웹 UI renderer·저장소를 추가하지 않았다. 앱 시작과 명시적인 새 탭에만 표시 자격을 부여하고 CLI·팝업·DevTools 등의 빈 문서를 덮지 않는다. 실제 선택·공간·탐색 세대·CEF 메인 프레임을 제출 직전에 확인하며, 기존 주소 해석과 해당 AI 소유 작업의 인계 경로를 사용한다. 같은 대상의 배경 갱신·오버레이 왕복은 초안을 보존하고 대상 변경은 초기화한다.

`tests/fixtures/start-page.html`과 `tests/preview_fixture.py`에 자체 HTTP 목적지와 내용이 있는 about:blank 팝업을 추가했다. Python·JavaScript 구문 검사는 통과했으며 모의 DOM 검사는 새로 실행하지 않았다. v1과 최종 v2는 각각 Rust 단위 검사 194개·전체 타깃 검사·format·release 빌드가 통과했다. 기존 테스트 전용 dead_code 경고 2개는 남는다. 최종 소스 해시는 `artifacts/start-page-v2-source.json`, 검사 근거는 `-rust-tests.log`, `-check.log`, `-fmt.log`, `-build.log`에 있다. 최종 앱 SHA-256은 `9ef0593acf202908e7bdd94f4da34a57f4d12a7be9aac7c637bf0fe6bdcaf021`, CLI는 `47070c0aca674370d4e4af0d98e957396b5816ff4eeb31cabfff4b9cdc991450`다.

v1 실제 화면에서는 사용자 중앙 입력 Enter의 HTTP 이동, 내용이 있는 about:blank 팝업 본문 표시와 네이티브 입력 전달, 배경 AI HTTP·CLI 읽기 중 초안 이어쓰기, 북마크 왕복 보존을 확인했다. 팝업 입력은 CLI read에서도 확인했다. AI가 맡은 네이티브 빈 탭의 잘못된 입력은 A/B 모두 active revision 1을 유지했고, 유효한 버튼 제출은 A만 paused revision 2로 바꿨다. 그러나 빈 Enter 뒤 포커스가 주소줄로 옮겨지고, AI 인계의 중간 화면 갱신이 주소줄을 편집 상태로 만들어 HTTP 완료 뒤에도 URL이 비어 있는 오류를 발견했다. `artifacts/start-page-v1-native.json`, `-popup-read.json`, `-ai-invalid-state.json`, `-ai-submit-state.json`에 구분했다. 이 v1에 CEF 회귀 32개를 실행하지 않았다.

최종 v2는 빈 제출의 실제 대상을 검증한 뒤 중앙 편집기로 복귀한다. 실제 nonempty 제출 동안만 임시 대상 하나를 보관해 같은 탭·공간의 중간 주소 포커스 이동을 막고 성공·실패 모두 반환 직후 해제한다. 성공 뒤 실제 선택이 같고 시작 화면·오버레이가 숨겨졌을 때 본문에 포커스를 준다. 주소 값이나 다른 편집 초안을 직접 덮지 않는다. AppKit의 공유 field editor는 해당 입력의 currentEditor와 실제 firstResponder 포인터가 같을 때만 상태 borrow 밖에서 정리한다.

최종 v2 실제 화면에서 빈 Enter 뒤 중앙 입력에 한글을 이어 붙일 수 있었고, 배경 빈 탭을 닫은 뒤에도 초안과 편집기가 유지됐다. 배경 AI HTTP 생성·완료 및 CLI read, 북마크·다운로드·통합 검색 왕복 후에도 같은 초안을 확인했다. 새 탭 단축키는 기존 상단 주소 포커스를 유지하고 선택 변경은 이전 중앙 초안을 지웠다. AI 빈 탭의 빈/잘못된 입력은 A/B active revision 1을 유지했으며, 유효한 버튼 제출 뒤 A만 paused revision 2·B active revision 1이고 실제 URL이 주소줄에 표시됐다. 같은 탭의 commit·문서·load 완료를 CLI 메타데이터와 대조했다. 두 빌드 모두 자체 HTTP가 완료됐으며 새 키체인 승인 질문을 하지 않았다. 변경 빌드 사이 권한 유지의 일반적인 해결로 확대하지 않는다. 근거는 `artifacts/start-page-v2-native.json`, `-first-http.json`, `-ai-invalid-state.json`, `-ai-submit-state.json`이다.

Computer Use가 사용자 직접 입력에 따른 변경을 감지해 앱 조작을 멈췄다가, 사용자의 “계속 검증해줘” 응답 후 같은 바이너리로 검사를 재개했다. 사용자 입력값은 검증 자료나 문서에 저장하지 않았다. 새 사용자 탭의 중앙 Enter로 자체 HTTP를 열어 실제 URL·본문을 확인했다. 내용이 있는 about:blank 팝업은 시작 화면에 가려지지 않았고 실제 클릭·붙여넣기를 CLI read로 대조했다. CLI reload는 epoch 2→5로 바꾸면서 이전 초안을 지우고 주소줄로 포커스를 돌렸다. 별도 빈 탭의 CLI scroll 뒤에는 시작 화면이 사라져 CEF 문서를 드러냈다. 사이드바 접기·펼치기에서도 중앙 초안·편집기·폼 배치를 확인했다. 한글 검사는 붙여넣기이며 IME 조합, 실제 최소 창 크기, 다크 모드, 외부 Google 검색·사이트, 메모리 비교·장시간 사용은 미검증이다.

같은 최종 start-page-v2에서 select 10개·tasks 6개·spaces 10개·single_window 6개, 총 32개 로컬 CEF 회귀가 모두 exit 0·빈 stderr로 통과했고 cleanupVerified도 true다. v1이나 이전 aside-toss-v2의 검사 수를 합산하지 않았다. 근거는 `artifacts/start-page-v2-native.json`, `-reload.json`, `-suite.json`, `-suite-state.json`이다. 임시 스크립트의 잘못된 import는 앱 접근 전에, 첫 정리 시도의 task info 응답 구조 오인은 변경 전에 실패했다. 확인한 result.task 구조로 고쳐 정상 정리했다. 이는 제품 검사 실패와 구분한다.

이전 앱 PID 4240과 v1 PID 13976은 CmdQ로 정상 종료(exit 0)했고 최종 PID 19977 하나가 실행 중이다. 두 빌드 모두 Cargo 중간 파일 4,332개·2.5GiB를 정리했다. 자체 서버 PID 11973과 후속 PID 23412도 SIGTERM 후 exit 0으로 종료했다. 재개 후 새 빌드는 하지 않았다. target 약 2.9MiB·dist 335MiB·검증 자료 17.6MiB·CEF 캐시 339MiB이며 최신 앱 번들 하나를 유지한다. 사용자 프로필과 원래 탭 1은 닫거나 삭제하지 않았다. 검증 탭을 모두 닫고 자체 작업 11개가 finished·잔여 탭/종료 대기 없음임을 확인했다. 원래 사용자 탭 1만 선택됐고 다운로드·권한·저장·wait·capture·탭 생성 대기는 모두 0이다. `artifacts/start-page-v2-native-cleanup.json`, `-final-state.json`, `-validation.json`에 최종 상태를 기록했다.

접힌 사이드바의 검색·새 탭 배경은 실제 화면에서 왼쪽 모서리가 잘린 듯 보였다. 소스의 배경 프레임은 x12~64pt로 음수가 아니며, 원인을 캡처나 AppKit 배치로 확정하지 못했다. 중앙 폼·입력 보존 검증과 구분하여 원본 화면과 실제 뷰·레이어 좌표를 대조할 후속 시각 점검으로 남긴다.

README·제품 계획·메모리 비용 문서와 `docs/start-page-design.md`를 갱신했다. 전체 목표는 계속 활성 상태이며 Naru 완성이나 Preseller 내장을 완료로 표시하지 않는다. 커밋·푸시·배포는 하지 않았다.

## 2026-10-13, Aside 구성·토스 표현의 1차 네이티브 UI 적용

요청한 디자인 방향을 `src/control_panel.rs`의 공통 스타일·사이드바·주소줄에 적용했다. 상단 검색 40pt → 공간 전환 → 현재 공간·38pt 세로 탭 순서로 정리하고, 하단 북마크·다운로드는 보조 행, 새 탭은 파란 40pt 주 버튼으로 구분했다. 툴바는 56pt, 주소 영역은 36pt, 주소·탭 제목은 13pt 시스템 폰트다. 중립 배경·파란 선택 표면·중간 굵기를 사용한다. 기존 216pt 사이드바·76pt 접힘, 행 재사용·주소 초안·AI 제어권·액션 검증은 유지한다. 고정 배경 뷰 세 개만 추가했으며 UI용 웹 renderer·worker·타이머·저장소는 추가하지 않았다. README·제품 계획·메모리 문서를 갱신했다.

첫 v1 실행에서 새 탭의 아이콘이 왼쪽 끝, 글자가 가운데에 떨어져 보였다. AppKit의 `imageHugsTitle` 계약을 확인한 뒤 새 탭 버튼에만 `setImageHugsTitle(true)`를 적용해 v2에서 아이콘·글자가 함께 가운데에 배치됨을 실제 화면으로 확인했다. 버튼과 둥근 배경은 형제 뷰로 유지해 부모 배경이 포커스 표시를 자르지 않게 했다. v1의 자체 HTTP·약 800pt 너비(1602×1104 스크린샷)의 펼침/접힘·검색·북마크와 탭 종료 뒤 북마크 입력 유지는 별도 결과다. 최종 v2의 창 경계 드래그는 크기를 바꾸지 못했으므로 최종 네이티브 최소 창 검증으로 합산하지 않는다.

최종 v2 소스에서 Rust 단위 검사 190개, 전체 타깃 Cargo 검사, format·diff 검사와 release 빌드가 통과했다. 최소 창에서 주소·상태·중지·닫기 영역이 분리되는 순수 기하 회귀를 추가했고 기존 찾기·복구 영역 검사도 새 높이로 확인했다. 기존 test-only dead_code 경고 두 개는 남는다. 근거는 `artifacts/aside-toss-v2-final-rust-tests.log`, `-final-check.log`, `-final-fmt.log`, `-build.log`다. 새 모의 DOM 검사는 하지 않았다.

같은 v2 앱의 로컬 CEF 선택 10개·작업 6개·공간 10개·단일 창 6개, 총 32개가 exit 0·빈 stderr·cleanup true로 통과했다. 실제 네이티브에서는 새 탭 생성과 주소 포커스, 배경 빈 탭을 닫은 뒤 주소 초안과 이어 쓰기, 한글 붙여넣기 검색과 Enter 복귀, 긴 한글 제목 말줄임·접근성 도움말, 접힌 탭 닫기, 공간 카드, 빈 다운로드 패널을 확인했다. 빈 AI 작업 A의 중지 버튼은 A만 paused revision 2로 바꾸고 B는 active revision 1을 유지했다. 이는 활성 웹페이지 조작 중 인계 검사와 구분한다. IME 조합 입력, 다크 모드, 키보드만으로 모든 버튼 순회, 외부 사이트, 다운로드 저장·권한 창, 비교 RAM·장시간 검사는 하지 않았다. 근거는 `artifacts/aside-toss-v2-native.json`, `-native-after-stop.json`, `-suite.json`, `-suite-state.json`이다.

최종 빌드 첫 HTTP는 5,041ms, 짧은 주소 재검사는 5,045ms에 WAIT_TIMEOUT이었다. 자체 서버에는 해당 요청이 도착하지 않았고 앱 스레드 825개 표본이 `SecItemCopyMatching → SecKeychainItemCopyContent → SecurityServer::ClientSession::decrypt`에서 대기했다. 보안 창을 도구로 확인하거나 승인하지 않았고 사용자에게 직접 확인을 요청했다. 이후 네이티브 검사 중 같은 앱·같은 바이너리에서 실제 HTTP commit·문서·load 완료를 확인해 나머지 검증을 이어갔다. OS 대기가 풀린 정확한 이유나 변경 빌드 간 키체인 권한 유지의 해결로 단정하지 않는다. 최초 실패와 후속 성공은 `-cli.jsonl`, `-startup-stack.txt`, `-http-recovered.json`, `-http-recovered-state.json`에 보존했다.

이전 메인 앱 88613·98551은 CmdQ로 정상 종료(exit 0)했고 최신 v2 메인 PID 4240 하나만 실행 중이다. 최종 앱 SHA-256은 `c00666472cca6ad069f511050cad84d9a0f11dcc08f673edea46950d52da64ff`다. 자체 서버 93300도 SIGTERM 후 exit 0으로 종료했다. 검사용 탭·작업을 정리해 원래 user 빈 탭 1과 선택만 남겼고 다운로드·권한·저장·wait·캡처·탭 생성 대기는 모두 0개다. 사용자 프로필을 보존했다. v1은 Cargo 중간물 4,848개·2.6GiB, v2는 4,332개·2.5GiB를 정리했다. 최종 target 약 2.9MiB·dist 335MiB·검증 자료 16.6MiB·CEF 캐시 339MiB다. `artifacts/aside-toss-v2-final-state.json`, `-validation.json`에 결과와 한계를 기록했다.

이 작업은 독립 Naru의 탐색 UI 1차 개선이다. 새 탭 시작 화면, 검색·북마크 내부 배치의 추가 정리, hover·키보드 접근성·최종 좁은 창/다크 검증, 빌드 간 키체인 대기와 실제 메모리 비교는 남아 있다. Naru 전체 완성과 Preseller 내장은 완료로 표시하지 않는다. 커밋·푸시·배포는 하지 않았다.

## 2026-10-13, 탐색 실패 복구 검증 완료와 Aside·토스 UI 방향 반영

독립 브라우저의 실패 복구를 개선했다. `src/page_recovery.rs`의 대상 값과 `src/browser.rs`의 실제 탭·공간·탐색 세대 검증을 연결하고, `src/control_panel.rs`에 96포인트 AppKit 복구 막대·주소 수정·새로 고침을 추가했다. 주소 수정은 포커스만 옮기며 명시적인 새로 고침은 기존 reload와 AI 제어권 인계 경로를 사용한다. 공간 목록·overlay에서는 숨기고 로딩 중 재시도와 오래된 대상을 거절한다. 별도 UI renderer·창·저장소를 추가하지 않았다. 렌더러 종료 분류도 구현했지만 실제 프로세스 종료 검증은 아직 하지 않았다.

변경 전 앱에서 Chromium의 ERR_EMPTY_RESPONSE와 CLI의 ERR_ABORTED가 다른 현상을 관찰하고, 자체 서버를 복구한 뒤 네이티브 ⌘R의 같은 주소 복구를 확인했다. recovery-v1 앱 SHA `cd4d478aed0e864830ebc187c0c613275b93f52d4d9279f630ff58960bd128bc`는 Rust 185개·전체 타깃·format 검사와 release 빌드를 통과했다. 첫 HTTP는 문서 생성 전 5,049ms WAIT_TIMEOUT과 새 SecurityAgent를 보였고, Computer Use의 `com.apple.SecurityAgent` 접근은 도구가 명시적으로 거절했다. 사용자에게 직접 확인을 요청했으며 허용 응답 후 같은 앱의 HTTP가 완료됐다. 사용자 인증정보를 명령·로그·파일에 넣거나 보안 접근 제한을 우회하지 않았다.

v1의 짧은 관찰을 사용한 로컬 CEF 복구 6개 검사는 통과했지만 실제 화면을 확인하자 복구 막대가 사라졌다. `artifacts/navigation-recovery-v1-app.log`는 -324 뒤 같은 URL의 browse-begin이 오류를 지우고 후속 -3을 기록하는 경로를 보였다. `-native.json`, `-native-failure-state.json`에 실패를 남겼다. `tests/navigation_recovery_smoke.py`의 관찰을 최소 12초로 늘리고 진행 중인 재시도와 완료 상태를 구분하자 같은 v1에서 오류 소실을 잡았다(exit 1). `-delayed-smoke.json`의 cleanup은 true이며 원래 user 탭 1을 보존했다. 짧은 검사 통과를 기능 완료로 확대하지 않는다.

v2는 같은 URL의 비사용자 후속 탐색에서 이전 실제 오류 하나를 내부 후보로 이동하고, 같은 URL의 ABORTED에서만 복원한다. 진행 중인 public error는 비워 `wait`가 이전 오류로 즉시 실패하지 않게 했다. 명시적인 native/CLI 탐색, 사용자 gesture, redirect/history, 다른 URL, 새 오류, 정상 commit, 오류 없는 종료, 중지와 렌더러 종료에서 후보를 지운다. CEF가 내부 오류 페이지 식별자를 제공하지 않아 같은 URL의 비사용자 스크립트 탐색과 엔진 재시도를 완전히 구별하는 구현은 아니다. opt-in 진단은 코드·세대·일치 여부·전이값만 기록하고 원문 URL·오류 문자열은 기록하지 않는다.

`tests/navigation_recovery_fixture.py`는 자체 loopback 실패/복구 fixture이며, 스모크는 정리 전 task의 tabs·closingTabs가 자체 생성 ID에만 속하는지 검사한다. 최종 소스 Rust 189개·전체 타깃 Cargo·format·diff·Python 문법 검사가 통과했다. 기존 test 전용 dead_code 경고 2개는 남는다. `artifacts/navigation-recovery-v2-final-rust-tests.log`, `-check.log`, `-fmt.log`가 근거다. v2 release도 성공했다. 앱 SHA는 `f7697af15e84619a758dc12ca5d4ec0bdc649d237f56f6702f67539f47fb4232`, CLI는 `63f9774beae38a982ed5dd52df81ea65fd301b52b9ab31c78827e940f9439f76`다. 같은 프로필의 첫 HTTP가 완료됐으며 v2 실행에는 별도의 사용자 승인 질문을 하지 않았다. 변경 빌드 간 키체인 권한 유지 전체를 보장하는 결과는 아니다.

v2의 확장 로컬 CEF 복구 6개가 통과했다. 사용자·AI 실패를 각각 12초 이상 관찰했으며 같은 URL 후속 browse의 transition 33554433, gesture/redirect false, -324 후보 뒤 -3 콜백에서도 원래 오류가 유지됐다. CLI reload는 같은 페이지를 복구했고 AI active·revision 및 사용자 선택도 유지했다. 진단 로그에는 CEF의 browser-info 응답 timeout도 남는다. 이번 수정은 Naru의 오류 보존과 명시적인 복구이며 그 엔진 내부 timeout의 근본 원인을 해결한 것으로 표시하지 않는다. `artifacts/navigation-recovery-v2-smoke.json`, `-app.log`에 근거가 있다.

네이티브 UI에서는 실패 막대가 계속 표시되고 주소 수정이 해당 URL을 선택함을 확인했다. 검색 overlay에서는 버튼이 숨겨지고 Esc 뒤 복원됐다. 서버를 정상화하고 막대의 새로 고침을 누르면 같은 사용자 탭에서 `Naru Recovery Restored`가 표시되고 막대가 사라졌다. AI A의 주소 수정 뒤 A/B 모두 active·revision 1을 유지했고, 복구 버튼은 A만 paused·revision 2로 바꾸고 B는 active·revision 1을 유지했다. 같은 AI 탭의 정상 제목·load 완료·오류 해제도 확인했다. 초기 검사 코드가 AI 탭에 전역 wait를 보내 TASK_REQUIRED로 거절된 기록은 그대로 남기고, 작업을 재개하지 않은 채 실제 화면과 전역 탭 메타데이터로 검증했다. `-native.json`, `-cli.jsonl`에 근거가 있다. 실패 페이지의 ⌘F는 준비되지 않았다는 안내를 반환했으므로 찾기와 복구 막대의 동시 표시를 실제로 검증했다고 기록하지 않는다.

기존 CEF select 10개·tasks 6개·spaces 10개·single_window 6개도 통과했다. 추가 navigation의 첫 실행은 bind만 한 소켓의 즉시 연결 거절을 가정하다 WAIT_TIMEOUT을 만났다. 앱과 무관한 자체 TCP probe는 같은 Mac에서 bind/no-listen 연결이 501ms 뒤 EAGAIN이고 listen 대조군은 0.405ms 연결 성공임을 보였다. `tests/navigation_smoke.py`, `tests/controls_smoke.py`를 요청 수신 뒤 응답 없이 연결을 닫는 자체 fixture로 바꾸고 요청 수신 여부·음수 오류 코드·문자열·주소 검증을 유지했다. 같은 앱에서 navigation 6개·controls 7개가 통과했고 204·중지·이력·동일 문서·복구를 확인했다. 최종 로컬 CEF 성공은 합계 51개이며 각 실행은 exit 0·빈 stderr·자체 탭 정리를 확인했다. 첫 실패와 OS probe는 별도 파일에 보존한다. 새 모의 DOM·외부 사이트·IME·좁은 창·다크 모드·실제 렌더러 종료·메모리 절감량 검증은 하지 않았다.

이전 앱 PID 71159와 v1 PID 81208은 각각 CmdQ 뒤 exit 0으로 종료했다. v1 빌드는 중간물 4,332개·2.5GiB, v2 빌드는 4,848개·2.6GiB를 정리했다. 마지막 target 2.9MiB·dist 335MiB·artifacts 16MiB·CEF 캐시 339MiB이며 dist에는 최신 앱 하나만 있다. 자체 fixture PID 74523도 SIGTERM 뒤 exit 0으로 종료했다. 최종 메인 앱 PID 88613 하나, 원래 user 빈 탭 1과 선택만 남았고 생성한 모든 작업은 finished·빈 소속 탭이다. 다운로드·저장·권한·wait·캡처·탭 생성 대기는 모두 0이다. 사용자 프로필은 보존했다. `-final-state.json`, `-validation.json`에 최종 경계를 기록했다.

작업 도중 사용자는 기능을 어느 정도 완성한 뒤 Aside 형식에 토스 디자인을 참고하자고 제안했다. 기존 실제 앱 비교와 새 공식 Aside·TDS·토스 UX Writing 자료를 확인해 `docs/product-plan.md`에 다음 UI 기준과 순서를 반영했다. 구성은 Aside의 왼쪽 탐색, 표현은 토스의 색·글자·간격·행·버튼·문구 원칙, 동작은 Naru의 사용자·AI 공간과 CLI다. 공개 Mobile/React Native 원칙은 AppKit에 맞게 적용하며 UI renderer나 TDS 실행 패키지를 추가하지 않는다. 제안 수치는 Naru용 시안이며 이번에 전체 화면 재설계를 구현한 것은 아니다. README·제품 계획·메모리 문서를 갱신했다. 커밋·푸시·배포는 하지 않았으며 Naru 전체 완성과 Preseller 내장은 미완료다.

## 2026-10-13, 최종 검색 빌드 승인 후 검증·재시작·정리 완료

사용자의 허용 응답 뒤 최종 tab-search-label-v1 앱 PID 68589와 SHA-256 `9a24e9181f909c07482cf26f610676be39b3b618990525b37d65faab3ac6eb88`가 유지됨을 확인했다. 재빌드나 제품 코드 변경 없이 미완료 검증을 이어갔다. 기존 자체 HTTP 탭 2는 commit·문서 생성·메인 load 완료 상태였고 실제 페이지가 표시됐다. 최초 5,039ms WAIT_TIMEOUT은 `artifacts/tab-search-label-v1-validation.json`과 첫 HTTP 로그에 보존한다. 이 빌드의 승인 대기는 해소됐다.

네이티브 UI에서 자체 페이지의 이탈 확인을 켜고 원래 user 탭 1로 돌아와 주소 초안을 입력했다. 배경 탭 2의 행 닫기 버튼을 누르면 beforeunload 확인창이 열렸다. Cancel은 탭을 유지하고 OK는 해당 탭을 닫았으며, 두 경로 모두 원래 선택과 주소 초안을 보존했다. 주소창을 다시 클릭하지 않고 이어 쓰기도 성공했다. `artifacts/tab-search-label-v1-approved-native.json`에 CUA 관찰과 실제 HTTP 상태를 기록했다. 앞선 같은 바이너리의 “새”·“새 탭” 검색 확인과 함께 최종 소스의 네이티브 결과로 사용한다. 한글 IME 조합 입력 검사는 아니다.

같은 최종 바이너리의 로컬 CEF 검사 select 10개·tasks 6개·spaces 10개·single_window 6개, 총 32개가 exit 0·빈 stderr·cleanup true로 통과했다. 검사 후 원래 user 빈 탭 1만 남았고 다운로드·권한·저장·wait·캡처·탭 생성 대기는 모두 0개였다. `artifacts/tab-search-label-v1-suite.json`, `-suite-state.json`, `-post-suite-state.json`에 결과를 남겼다. Rust 단위 검사 179개·독립 검색 6개·전체 타깃 검사·format은 이전에 통과한 같은 소스의 결과이며 이번에 다시 실행하지 않았다. 새 모의 DOM 검사나 외부 사이트 검증도 실행하지 않았다.

CmdQ로 앱의 정상 종료(exit 0)를 확인한 뒤 같은 SHA·같은 tasks-phase1 프로필로 재실행했다. 새 메인 PID는 71159이며 추가 승인 요청 없이 자체 HTTP 페이지가 commit·문서 생성·load 완료에 도달했다. 재시작 뒤 SecurityAgent는 관찰되지 않았다. 이는 같은 바이너리 재시작의 검증이며, 코드가 바뀐 두 빌드 사이의 인증서 서명·키체인 권한 유지까지 검증한 것은 아니다.

재시작 검사용 탭 2를 닫고 자체 fixture PID 65830에 SIGTERM을 보내 exit 0으로 종료했다. 원래 user 빈 탭 1의 선택, 빈 작업 목록, 모든 대기 0개를 확인했다. 이전 앱은 종료됐고 최신 앱 PID 71159 하나만 실행 중이다. 사용자 프로필은 보존했고 새 빌드 중간물은 만들지 않았다. target 2.9MiB·dist 335MiB·검증 자료 15MiB·CEF 캐시 339MiB이며, 앞선 빌드의 중간물 671.5MiB 정리 결과를 유지한다. 정리·재시작 증거는 `artifacts/tab-search-label-v1-restart-http.json`, `-final-state.json`, `-final-validation.json`이다.

README·제품 계획·메모리 문서의 최종 HTTP 승인 대기와 CEF 미실행 상태를 실제 결과로 갱신했다. 이번 탭 행·검색 수정의 최종 로컬 검증은 마쳤다. 메모리 절감 측정, 좁은 창, 키보드만으로 행 닫기, 외부 사이트·장시간 사용, 실제 인증서로 변경 빌드 간 권한 유지와 Preseller 내장은 남아 있으며 Naru 전체 완성으로 표시하지 않는다. 커밋·푸시·배포는 하지 않았다.

## 2026-10-13, 승인 후 탭 닫기 검증과 새 탭 검색 최종 빌드

사용자의 키체인 허용 응답 뒤 같은 tab-rows-v1 앱 PID 56917에서 검증을 재개했다. 앱 SHA-256 `2a5b4ae280cf0d195e9e388279ba58174a6a7d0147cc06310e36ef7923d1c7bb`는 유지됐다. 자체 fixture를 새로 실행하고 HTTP 탭 11의 commit·문서 생성·load 완료와 제목을 확인했다. 직전 승인 대기는 이 바이너리에서 해소됐다.

네이티브 UI로 자체 페이지의 이탈 확인 체크박스를 클릭한 뒤 원래 user 탭 1로 돌아와 주소 초안을 입력했다. 배경 탭 11의 행 닫기 버튼으로 beforeunload 확인창이 열렸고, Cancel 뒤 탭이 유지됐다. 다시 닫아 OK를 누르면 탭 11이 제거됐다. 두 경우 모두 user 탭 1의 선택과 초안이 유지됐으며 주소창을 다시 클릭하지 않고 이어 쓸 수 있었다. 로컬 CEF 회귀 select 10개·tasks 6개·spaces 10개·single_window 6개, 총 32개가 exit 0·빈 stderr·cleanup true로 통과했다. 마지막 status의 다운로드·권한·저장·wait·캡처·탭 생성 대기는 모두 0개였다. 근거는 `artifacts/tab-rows-v1-approved-native.json`, `-approved-native-state.json`, `-suite.json`, `-suite-state.json`이다. 외부 사이트 검증과 구분한다.

이미 수정·검사한 `src/tab_search.rs`와 `src/control_panel.rs`의 표시 제목 검색을 release 앱에 반영했다. “새 탭” 별칭 검색과 기존 원래 제목·URL·공간 검색을 함께 유지한다. 같은 소스의 Rust 단위 검사 179개·전체 타깃 Cargo 검사·format 검사와 독립 검색 검사 6개는 앞선 로그의 결과이며 이번에 재실행한 것으로 세지 않는다. 새 모의 DOM 검사는 실행하지 않았다. `scripts/dev.sh`, `scripts/bundle-macos.py`의 선택 서명자 옵션도 포함하되 실제 빌드는 기본 ad hoc을 사용했다. 인증서를 선택·생성하거나 키체인 권한을 도구로 변경하지 않았다.

이전 앱을 CmdQ로 정상 종료(exit 0)한 뒤 최종 빌드가 성공했다. 최종 앱 SHA-256은 `9a24e9181f909c07482cf26f610676be39b3b618990525b37d65faab3ac6eb88`, CLI는 `5500a7107d94c94638ff4dd9cbecb52b17b2b237dacd83eeccba513af6693ec6`다. 최종 앱의 네이티브 검색창에 “새”, “새 탭”을 각각 붙여 넣자 빈 탭 한 개가 표시됐고 Enter로 원래 탭을 선택했다. 이는 한글 붙여넣기 검사이며 IME 조합 입력 검사가 아니다. 근거는 `artifacts/tab-search-label-v1-build.log`, `-validation.json`과 CUA 관찰이다.

최종 앱 PID 68589에서 자체 HTTP 탭 2의 첫 wait는 5,039ms 뒤 WAIT_TIMEOUT으로 끝났다. commit·문서 생성 전이며 앱 시작 직후 새 SecurityAgent PID 68597이 확인됐다. 새 서명에 대한 키체인 승인 대기로 의심해 사용자에게 직접 허용을 요청했다. 보안 창을 대신 확인하거나 승인하지 않았다. 최종 바이너리의 HTTP·CEF 회귀 32개는 아직 통과하지 않았으며 이전 v1의 결과로 대체하지 않는다. 같은 바이너리를 검증할 실행기는 `artifacts/tab-search-label-v1-suite-run.py`에 준비했다.

빌드는 Cargo 중간물 1,263개·671.5MiB를 정리했다. target 2.9MiB, dist 335MiB, 검증 자료 15MiB, CEF 캐시 339MiB다. 이전 앱 PID 56917은 종료했고 최신 앱 하나만 실행 중이다. 승인 후 검증을 위해 자체 fixture PID 65830과 검사용 배경 탭 2를 유지하며 기존 프로필과 원래 user 탭 1을 보존하고 선택도 1로 복원했다. README·제품 계획·메모리 문서를 실제 바이너리별 결과로 갱신했다. 새 메모리 절감 측정·좁은 창·키보드만으로 행 닫기·외부 사이트·실제 인증서 서명 간 권한 유지·Preseller 내장은 미검증이다. 이번 변경의 커밋·푸시·배포는 하지 않았다.

## 2026-10-13, 최신 소스 전체 검사와 키체인 대기 정리

앞선 턴은 검색 소스 수정·서명자 지정 옵션·네이티브 패널 검사·키체인 스택 증거를 만든 진행으로 분류한다. 이번에는 같은 앱 PID 56917과 fixture PID 47769가 살아 있음을 먼저 확인했다. 자체 HTTP 탭 2는 여전히 committed false·documentAvailable false였으며 원래 user 탭 1의 선택을 보존했다. `artifacts/tab-rows-v1-third-turn-state.json`에 시작 상태를 남겼다.

새 검색 수정까지 포함한 최신 소스로 Rust 단위 검사 179개, 전체 타깃 Cargo 검사, format·diff 검사가 통과했다. 기존 test 전용 dead_code 경고 2개는 남는다. 근거는 `artifacts/tab-search-label-v1-rust-tests.log`, `-check.log`, `-fmt.log`다. 앞선 독립 검색 6개·서명 모의 검사와 구분한다. 새 모의 DOM·CEF HTTP·네이티브 검색·beforeunload·외부 사이트 검증이나 실제 인증서 서명은 실행하지 않았다. release 앱도 다시 만들지 않았으므로 새 검색 수정은 현재 실행 앱에 아직 없다.

더 이상 사용하지 않는 자체 HTTP 탭 2를 닫고 fixture PID 47769에 SIGTERM을 보내 exit 0으로 정상 종료했다. `scripts/cleanup-build.py --profile release`는 검사 중간물 3,073개·1.8GiB를 삭제하고 기존 release 실행 파일 세 개를 보존했다. 원래 user 빈 탭 1과 선택만 남았으며 다운로드·권한·저장·wait·캡처·탭 생성 대기는 0개다. `artifacts/tab-rows-v1-blocked-cleanup-state.json`과 `artifacts/tab-search-label-v1-build-cleanup.log`에 결과를 남겼다. target 2.9MiB, dist 335MiB, 검증 자료 14MiB다. 사용자 프로필은 보존했다.

마지막에 살아 있는 같은 앱을 1초 sample해 `SecItemCopyMatching → SecKeychainItemCopyContent → SecurityServer::ClientSession::decrypt → mach_msg`에서 813개 표본이 계속 대기하는 것을 확인했다. 원본은 `artifacts/tab-rows-v1-blocked-stack.txt`다. 사용자 보안 창을 읽거나 권한을 대신 바꾸지 않았다. 앞선 두 턴과 이번 턴에 같은 OS 키체인 조회 대기가 이어졌다. 별도로 가능한 소스 수정·전체 검사·모의 서명 검사·빈 탭 UI 검사는 마쳤으며, 남은 HTTP·beforeunload·새 앱 적용 검증은 사용자 확인 또는 OS 응답 변화가 필요하다. 목표 전체는 미완료이며 동일 조건의 세 번째 확인 뒤 blocked로 전환하는 조건을 충족했다.

최신 메인 앱 PID 56917 하나는 그대로 유지한다. 앱 SHA-256 `2a5b4ae280cf0d195e9e388279ba58174a6a7d0147cc06310e36ef7923d1c7bb`, CLI `5500a7107d94c94638ff4dd9cbecb52b17b2b237dacd83eeccba513af6693ec6`가 정리 전후 일치한다. 이후 진행할 때 같은 앱에서 자체 fixture를 새로 시작해 HTTP부터 확인한다. 같은 브랜치의 변경은 여전히 미커밋이며 커밋·푸시·배포는 하지 않았다. README·제품 계획에 최신 소스 검사 범위와 현재 실행 앱의 차이를 반영했다. 문서 리뷰에서 서명의 기본값 설명과 CLI 종료 뒤 패널 포커스 검사를 행별 닫기 버튼 검사와 구분하도록 보정했다.

## 2026-10-13, 새 탭 검색 수정·패널 포커스 확인·서명자 선택 준비

앞선 탭 행 변경은 실제 코드·빌드·빈 탭 검증을 진행한 작업으로 분류한다. 이번 시작에 동일 앱 PID 56917과 자체 fixture PID 47769의 생존을 확인했으며 HTTP 탭 2는 여전히 commit 전이었다. 같은 바이너리의 미완료 HTTP 검사를 새 앱으로 대체하지 않고 현재 가능한 네이티브 검사를 진행했다.

실행 중인 tab-rows-v1에서 통합 검색의 현재 자체 탭 9를 CLI로 닫은 뒤 about:blank 입력을 이어 썼고, 검색 결과 2개와 입력 포커스가 유지됐다. 북마크 검색 중 자체 배경 탭 8을 닫은 뒤 입력을 이어 쓰고, 다운로드 패널의 닫기 버튼에 포커스를 둔 채 자체 배경 탭 10을 닫은 뒤 Space로 패널을 닫았다. 북마크를 추가하거나 파일을 저장하지 않았다. `artifacts/tab-rows-v1-followup.json`에 관찰을, `-followup-state.json`과 `-cli.jsonl`에 상태를 기록했다. Shift+Tab은 현재 환경에서 주소창 포커스를 벗어나지 않았고 한 번의 창 경계 drag도 크기를 바꾸지 못했으므로 키보드 전용 행 닫기·좁은 창을 성공으로 기록하지 않는다. 시스템 설정은 바꾸지 않았다.

같은 앱에서 “새 탭” 표시를 “새”로 검색하면 결과가 0개이고 about로는 결과가 나타나는 차이를 발견했다. `src/tab_search.rs::display_title`을 추가해 sidebar·검색 결과 표시와 검색 별칭을 일치시켰다. `src/control_panel.rs`의 두 표시 경로도 이 함수를 사용한다. 원래 제목·URL·공간의 검색을 보존하고 표시가 다른 경우 고정된 짧은 별칭만 참조한다. 기존 query 200자·결과 40개·필드 4,096자 상한을 유지한다. 일반 제목의 중복 복사·새 캐시·프로세스는 추가하지 않았다. 독립 검색 모듈의 회귀 6개와 format·diff 검사가 통과했다. `artifacts/tab-search-label-v1-tests.validation.json`에 rustc 단독 검사임을 기록했다. 새 소스는 이전 Rust 178개 검사와 release 이후 변경이며 이번에는 전체 Cargo 검사·release 빌드·네이티브 검색 재검증을 하지 않았다.

HTTP 대기를 추정만으로 설명하지 않기 위해 자체 네트워크 보조 PID 56925와 메인 PID 56917을 각각 1초 sample했다. 메인 앱의 ThreadPoolForegroundWorker 6991989는 821개 표본 모두 `SecItemCopyMatching → SecKeychainItemCopyContent → SecurityServer::ClientSession::decrypt → mach_msg`에서 대기했다. 실제 키체인 조회 응답 대기는 확인했지만 항목 값·정확한 대상 item·보안 창은 읽지 않았으며 모든 HTTP 대기와의 인과를 확정한 것은 아니다. 원본은 `artifacts/tab-rows-v1-browser-stack.txt`와 `-network-stack.txt`에 있다. CEF 내부의 제거된 심볼 대신 OS Security 함수 경로를 근거로 삼는다.

`scripts/bundle-macos.py`가 항상 ad hoc 서명을 쓰던 경로에 `--signing-identity`를 추가하고 `scripts/dev.sh`에 명시된 `NARU_CODESIGN_IDENTITY` 전달을 연결했다. 기본 '-'는 유지하고 빈/공백 값은 거절한다. 모든 중첩 Mach-O·framework·helper·CLI·앱에 같은 값을 전달하며 서명 실패를 숨기거나 ad hoc으로 대체하지 않는다. 기존 strict 검증, 이전 앱 종료, 검증된 번들 교체·복구 순서는 유지했다. 인증서를 조회·생성·선택하거나 키체인 권한을 수정하지 않았다. 실제 안정 서명이나 두 번의 변경 빌드 사이 권한 유지 성공으로 기록하지 않는다. 모의 subprocess 6개 검사 그룹과 환경변수 전달 5가지 경우, Bash 문법 검사를 통과했으며 두 스크립트의 독립 읽기 리뷰에서도 추가 수정 필수 결함을 찾지 못했다. `artifacts/signing-identity-v1-validation.json`은 실제 도구 실행 기록에서 결과를 정리한 자료이며 원본 임시 검사 스크립트나 별도 출력 파일은 보존하지 않았다.

모의 DOM·새 HTTP CEF 회귀·beforeunload·외부 사이트 검사는 미실행이다. 새 앱을 빌드하지 않아 메인 PID 56917, 앱 SHA `2a5b4ae280cf0d195e9e388279ba58174a6a7d0147cc06310e36ef7923d1c7bb`, CLI SHA `5500a7107d94c94638ff4dd9cbecb52b17b2b237dacd83eeccba513af6693ec6`는 유지된다. 새 자체 빈 탭 8~10은 모두 정리했고 원래 user 탭 1과 선택, HTTP 대기 탭 2만 남았다. 다운로드·권한·저장·wait·캡처·탭 생성 대기는 0개다. 검색 검사 실행 파일 1.2MiB는 성공 결과와 해시를 남긴 뒤 삭제했으며 모의 서명 fixture도 정리했다. target 2.9MiB, dist 335MiB, 검증 자료 14MiB다. 사용자 프로필·데이터는 보존했다. 같은 HTTP 검사와 사용자 확인을 위해 기존 앱과 fixture 한 개만 유지하며 README·제품 계획을 갱신했다. 커밋·푸시·배포는 하지 않았고 전체 목표는 진행 중이다.

## 2026-10-13, 세로 탭 행 닫기와 AppKit 재사용 — HTTP 검증 대기

독립 Naru의 일상 사용성을 개선하는 범위에서 `src/control_panel.rs`의 각 탭 행에 닫기 버튼을 추가했다. 펼친 행의 제목·아이콘과 접힌 76포인트 사이드바의 선택/닫기 영역을 구분했다. 탭 ID와 공간이 같으면 AppKit 컨트롤을 재사용하고 제거된 행의 참조는 해제한다. 별도 UI renderer, 프로세스, 반복 타이머나 영속 저장소를 추가하지 않았다. 실제 메모리 절감량은 측정하지 않았다.

행의 현재 버튼 포인터와 공간을 확인하고, `src/browser.rs`에서 실제 탭 소속을 닫기 직전에 다시 검사한다. adopt/release 뒤 화면에 잠시 남은 행으로 다른 공간의 탭을 닫거나 AI를 중지하지 않게 했다. 배경 행 닫기는 주소 초안을 보존하며 같은 AI 작업 인계의 정확히 한 세대 증가만 임시로 반영한다. 실제 탐색·같은 URL reload·선택 변경은 이 예외로 무시하지 않는다. 작업 인계와 CEF CloseBrowser(false)의 기존 이탈 확인 경로를 유지한다.

Rust 단위 검사 178개, 전체 타깃 Cargo 검사, format·diff 검사와 release 빌드가 통과했다. 추가 순수 회귀 4개는 행 배치, 무효 행, 실시간 공간 검증, 제한된 주소 인계 예외를 확인한다. 기존 test 전용 dead_code 경고 2개는 남는다. `artifacts/tab-rows-v1-rust-tests.log`, `-check.log`, `-fmt.log`, `-build.log`에 근거를 남겼다. 앱 SHA-256은 `2a5b4ae280cf0d195e9e388279ba58174a6a7d0147cc06310e36ef7923d1c7bb`, CLI는 `5500a7107d94c94638ff4dd9cbecb52b17b2b237dacd83eeccba513af6693ec6`다.

같은 앱의 네이티브 빈 탭 검사에서 펼친/접힌 배경 행 닫기 뒤 현재 선택, 입력한 주소와 입력 포커스를 유지했다. 닫은 뒤 다시 포커스하지 않고 문자를 이어 써서 확인했다. 선택한 자체 탭 닫기와 접힌 행의 아이콘/닫기 영역도 확인했다. 자체 AI A 공간에서 선택한 탭 5의 주소를 편집하며 형제 탭 6을 닫자 A만 paused revision 2가 됐고 B는 active revision 1을 유지했다. 선택 5와 주소 편집도 유지됐다. `artifacts/tab-rows-v1-native.json`, `-after-ai-close.json`, `-cli.jsonl`에 범위와 CLI 결과를 남겼다. 처음 CLI로 about:blank를 열려던 진단은 HTTP(S) 전용 계약으로 거절됐으며 이후 native Cmd+T로 만들었다. 이를 제품 결함으로 세지 않는다.

새 바이너리의 첫 로컬 HTTP 탭 2는 5초 wait에서 WAIT_TIMEOUT이고 commit·documentAvailable은 false였다. 앱 PID 56917 시작 직후 SecurityAgent PID 56927을 확인했다. 기존 빌드에서 사용자가 보안 창이 없다고 답한 이후 만든 새 바이너리임을 설명하고 직접 확인을 요청했다. 보안 창을 도구로 읽거나 승인하지 않았다. HTTP 페이지의 beforeunload·제목 갱신·기존 CEF 회귀 32개는 아직 실행하지 않았다. 새 모의 DOM·외부 사이트 검사, 최대 탭 부하와 비교 메모리 측정도 하지 않았다. 실행기는 새 SHA에 고정해 준비했으나 준비를 통과로 기록하지 않는다.

이전 앱 PID 44130은 정상 종료(exit 0)했고 최신 메인 PID 56917 하나만 남았다. 빌드에서 중간물 4,332개·2.5GiB를 정리했으며 dist는 최신 앱 하나, target/release는 실행 파일 세 개다. 현재 target 2.9MiB, dist 335MiB, 검증 자료 14MiB, CEF 캐시 339MiB다. 자체 빈 탭 3~7과 두 AI 작업은 정리했다. 원래 user 탭 1과 선택, 프로필을 보존하고 입력했던 검증 초안도 지웠다. 다운로드·권한·저장·wait·캡처·탭 생성 대기는 0개다. 같은 바이너리에서 확인을 이어가기 위해 자체 HTTP 탭 2와 fixture PID 47769/포트 56766, 최신 앱만 유지한다. README·제품 계획·메모리 문서를 갱신했고 이번 후속 변경은 커밋·푸시·배포하지 않았다. 앞선 비공개 업로드 완료 상태와 구분하며 전체 Naru 완성을 선언하지 않는다.

## 2026-10-13, 권한 v2의 실제 허용과 사용자 요청 재개 검사

직전 개발 턴은 소스 변경과 실제 CEF 검사, 비공개 업로드까지 진행한 상태다. 새 목표 턴에서 Git 작업 폴더가 깨끗한 상태와 기존 앱 PID 44130, 원래 user 공간의 빈 탭 1, 다운로드 대기 0개를 다시 확인했다. 추가 빌드 없이 같은 권한 v2 바이너리로 남은 네이티브 검사를 이어갔다.

직접 만든 로컬 사이트의 권한 카드에서 허용을 누르자 두 번째 파일도 저장 위치 대기로 들어갔다. 네이티브 저장창에서 각 경로를 선택해 131,072바이트와 8,388,608바이트 파일을 저장했다. 두 파일 모두 고정 fixture 원본 바이트와 일치했다. 작은 파일 SHA-256은 `59f410ae5e17962412e2aed4f815918f634932f2abf084f00bb638c4db017850`, 큰 파일은 `7d212b9c884f5c77896de960ae17cc341cda43b14d6a971f34ca29ebd4badf7f`다. 사용자 공간과 AI active revision 1을 보존했다. 완료 기록 지우기 후에도 파일과 해시가 유지됐고 확인한 자체 검증 파일 두 개만 삭제했다.

다른 로컬 origin의 자체 작업을 paused revision 2로 인계한 뒤 본문의 두 파일 연속 요청 버튼을 native 클릭했다. 링크 이벤트는 합성이지만 사용자 activation은 true였다. AI를 revision 3으로 재개하면 권한 대기 1개는 사라지고 먼저 접수한 사용자 파일의 저장 대기 1개는 유지됐다. 기존 paused 다운로드의 scope None 계약과 일치한다. 저장 대기를 포함한 active 개수는 실제 전송 중 개수와 같지 않다. 작업 종료 뒤 모든 다운로드 대기와 임시 탭을 정리하고 원래 user 탭 1로 돌아왔다.

근거는 `artifacts/download-permission-v2-native-followup.json`이다. 외부 사이트, 재개 뒤 첫 사용자 파일의 실제 저장, 모든 늦은 콜백 경합이나 사이트 권한 해제 UI는 검증하지 않았다. 이번 로컬 origin 허용은 CEF 프로필 설정에 남으며 별도 프로필 편집으로 제거하지 않았다. README, 제품 계획, 다운로드 설계와 메모리 문서를 실제 결과로 갱신했다. 후속 탭 UI 작업을 위해 최신 앱과 자체 fixture 한 개를 유지하며 추가 커밋과 푸시는 하지 않았다. 같은 앱에서 자체 탭을 정리한 뒤 RSS 세 표본은 모두 합계 722.094MiB였다. 공유 매핑이 중복되는 값이며 새로운 앱이나 최적화 전후 비교가 아니다. 원본은 `artifacts/download-permission-v2-after-native-memory.json`에 있다.

추가 실제 CEF 용량 검사에서는 파일과 저장 대기 4개가 찬 뒤 다섯 번째 요청이 추가되지 않았다. 최초 하네스는 파일 4개와 권한 4개가 동시에 생길 것으로 잘못 가정해 실패했다. 실제로 `CanDownload`의 파일 상한 검사가 후속 권한 요청 전에 거절하므로 관찰값은 파일 4개와 권한 3개였다. 이 호출 흐름을 확인한 별도 실행은 성공했고 자체 작업 5개를 종료해 원래 사용자 탭과 대기 0개를 보존했다. `artifacts/download-permission-v2-file-capacity.json`에 결과를 남겼으며 권한 큐 자체의 4개 경계에 대한 네이티브 검증으로 확대하지 않는다.

## 2026-10-13, 연속 다운로드 권한 처리와 비공개 업로드 준비

독립 브라우저 개발을 이어가며 이전에 남긴 연속 합성 다운로드 미접수를 조사했다. 기존 v4 바이너리의 자체 로컬 페이지에서 첫 파일만 서버에 도착하고 두 번째 파일은 클릭 성공 응답 뒤에도 요청되지 않는 현상을 재현했다. 이후 실제 CEF MULTIPLE_DOWNLOADS 콜백의 진입을 확인했다. 최초 재현 근거는 `artifacts/download-permission-baseline.json`이다.

`src/download_permissions.rs`와 `src/download_permission_runtime.rs`에 최대 4개의 권한 대기를 추가했다. 정확한 MULTIPLE_DOWNLOADS 요청만 받고 origin과 출처를 제한된 길이로 보관한다. 허용은 해당 사이트 설정을 저장하고 이번 요청 취소는 IGNORE로 설정 변경 없이 취소한다. `src/browser.rs`에서 문서와 최초 소유권을 재검증하고 탐색, 중지, 인계, 재개, adopt, release, 종료와 CEF dismiss를 연결했다. 콜백은 큐에서 제거한 뒤 borrow 밖에서 호출한다. CLI status에는 다운로드, 저장 위치 대기, 권한 대기 개수를 추가했다.

첫 구현의 실제 검사에서 숨긴 Chromium 요청이 인계 뒤 늦게 전달되면 새 소유자의 권한 요청으로 보이는 결함을 발견했다. 최초 CanDownload 시점의 소유권을 보관하고 제어권 변경 뒤에는 같은 문서에서 다시 허용하지 않도록 수정했다. 실제 문서 commit에서만 provenance를 초기화한다. 다운로드와 HTTP 204로 끝나는 provisional 탐색은 실제 문서 교체와 구분한다. 네이티브 화면에서 뒤집혀 표시되던 카드의 자식 좌표도 수정했다.

`src/control_panel.rs`의 기존 다운로드 패널에 origin, 요청 출처, 다른 공간과 공유되는 사이트 설정의 의미, 허용과 이번 요청 취소 버튼을 연결했다. 자동으로 패널이나 공간을 열지 않으며 닫힌 패널에서는 개수만 읽는다. 카드 참조는 닫을 때 해제하고 단조 증가 토큰과 실제 버튼을 함께 확인해 오래된 동작을 거절한다. 별도 창, 웹 renderer, worker, DB는 추가하지 않았다. README, 다운로드 설계와 제품 계획을 갱신했다.

최종 v2의 Rust 단위 검사 174개, 전체 타깃 Cargo 검사, format과 diff 검사, release 빌드가 통과했다. 기존 test 전용 dead_code 경고 2개는 남는다. 근거는 `artifacts/download-permission-v2-rust-tests.log`, `-check.log`, `-fmt.log`, `-build.log`다. 앱 SHA-256은 `8f6ca43ccee08f37594d366b474530de9b4b7626814126db832834a474d08be3`, CLI는 `c530aca66d43799ea254e052bc1557ade91f7cfeb1eab2be1cf4582530152d5e`다. 새 모의 DOM 검사는 실행하지 않았다.

v2 첫 실제 검사는 로컬 HTTP commit 전 WAIT_TIMEOUT으로 끝났으며 새 SecurityAgent를 확인해 직접 승인을 요청했다. 뒤이어 같은 바이너리에서 HTTP complete와 commit을 확인해 검증을 재개했다. 두 번째 하네스는 인계와 탐색 검사를 통과한 뒤 paused 탭의 unscoped read에서 TASK_REQUIRED로 실패했다. 제품의 정상 보호를 잘못 사용한 하네스였으므로 자동 resume 사례를 제외하고 미실행 항목으로 기록했다. 최초 실패 자료는 유지했다.

수정한 `tests/download_permissions_smoke.py`는 실제 로컬 CEF에서 인계, 문서 탐색, 사용자 탭 adopt, 숨긴 요청의 인계 4가지를 통과했다. 정리 검사까지 checks 5개, exit 0, stderr 0, cleanupVerified true다. 작업을 직접 표시해야 숨긴 Chromium 요청이 CEF 콜백에 도달하는 조건을 명시했다. 배경 요청의 자동 알림 성공으로 세지 않는다. `artifacts/download-permission-v2-smoke-final.json`에 원래 사용자 탭과 선택 보존, 대기 0개, 자체 서버 종료를 기록했다. 앱 로그에서 adopt와 숨긴 요청 인계 후의 실제 늦은 OnShow 진입도 확인했다.

동일 v2에서 기존 회귀 select 10개, tasks 6개, spaces 10개, single_window 6개, 총 32개가 exit 0으로 통과했다. 각 검사와 실행기의 정리 확인도 통과했다. 결과는 `artifacts/download-permission-v2-suite.json`, `-suite-state.json`, `-runner.log`다. 합성 CLI 입력을 사용하는 로컬 CEF 검사이며 외부 서비스 검증이 아니다.

네이티브 UI 확인에서는 자체 attachment 링크의 첫 파일이 저장 대기에 들어갔다. 두 번째 클릭 직후에는 PAGE_LOADING이었으나 이후 기존 문서가 유지되고 권한 콜백 mask 16384와 대기 1개가 확인됐다. 사용자 공간으로 돌아와도 대기는 유지됐다. 최종 v2 화면에서 카드 제목, origin, 출처, 설명, 버튼이 위에서 아래 순서로 배치됐으며 이번 요청 취소를 누르면 카드와 권한 대기가 사라지고 닫기 버튼으로 포커스가 복귀했다. 허용 클릭, 그 뒤 실제 파일 저장, paused 사용자 요청의 native 입력 후 재개는 이번 v2에서 실행하지 않았다. 이를 위한 로컬 fixture 버튼은 준비했지만 성공 결과로 세지 않는다.

검증용 작업과 탭은 모두 정리했고 원래 사용자 빈 탭 1, user 공간, 다운로드와 권한과 저장 대기 0개, wait와 캡처와 탭 생성 예약 0개를 확인했다. 자체 fixture PID 37571은 SIGTERM 뒤 exit 0으로 종료했다. 이전 앱은 정상 종료했고 최신 v2 메인 PID 44130 하나를 유지한다. 빌드 때마다 중간물 4,332개와 2.5GiB를 정리했으며 dist에는 최신 앱 하나, target/release에는 실행 파일 세 개만 남았다. target 2.8MiB, dist 335MiB, 검증 자료 13MiB, CEF 캐시 339MiB다. 이번 v2에서는 다운로드를 디스크에 저장하지 않았고 사용자 프로필과 기존 탭을 보존했다. 새 메모리 측정이나 같은 바이너리 재시작 검사는 하지 않았다.

숨긴 Chromium 권한 요청은 CEF에 전달되기 전에는 앱에서 알 수 없다. 대기 상한 4개는 앱이 받은 콜백에만 적용된다. 제어권 변경 뒤 같은 문서의 새 권한 요청도 reload가 필요할 수 있다. 권한 해제 설정 UI, 모든 수명 경합과 실제 동시 상한, 허용 뒤 저장, 외부 사이트, Preseller 내장과 전체 Naru 완성은 남아 있다.

비공개 GitHub 업로드 요청에 따라 `FEABE/naru-browser`의 PRIVATE와 ADMIN 권한, 개발 브랜치의 기존 원격 커밋 `37a8f549b28887cc5c3d91a0c4590e57753afb25`를 다시 확인했다. 변경 11개와 추적 파일을 점검했으며 프로필, 쿠키, DB, 비밀 파일, 앱 번들과 생성물은 업로드 대상에 없다. 로컬 검증 자료는 ignore된 artifacts에 유지한다. 소스, 테스트, 문서를 기존 `codex/navigation-readiness`에 올리는 범위이며 main 변경이나 배포는 포함하지 않는다. 소스 커밋 `9e0d6ba9d34640583a9ab844c65969eb269c9256`의 푸시가 성공했고 로컬 HEAD와 원격 개발 브랜치 SHA의 일치를 확인했다. GitHub 재조회에서도 `isPrivate: true`, `visibility: PRIVATE`였다. 이 업로드 확인 문장도 같은 개발 브랜치에 후속 문서 커밋으로 반영한다.

## 2026-10-13, GitHub 비공개 업로드 상태 확인

현재 프로젝트를 GitHub에 비공개로 올려 달라는 요청에 따라 로컬과 원격을 조회했다. `FEABE/naru-browser`는 `visibility: PRIVATE`, `isPrivate: true`이며 현재 계정의 권한은 ADMIN이다. 개발 브랜치 `codex/navigation-readiness`의 원격 커밋과 로컬 HEAD는 모두 `93ae7e91eb0f8c4c1d3f1b4f7103c41971b83c4c`로 일치했다. 소스는 이미 업로드된 상태이고 시작 시 미커밋 변경은 없었다.

이번 변경은 이 작업 기록뿐이다. 제품 코드와 공개 범위를 변경하지 않는다. 기본 브랜치 main은 유지하고 같은 개발 브랜치에 확인 기록을 반영한다. `git status`, `git diff --check`, GitHub 공개 범위와 원격 커밋 조회를 실행했다. 최초 제한 환경에서는 GitHub 연결이 실패했으며 네트워크 권한을 적용한 조회에서 위 결과를 확인했다.

앱 동작을 바꾸지 않아 Rust 단위 검사, 모의 DOM 검사, 로컬 CEF 통합 검사, 네이티브 UI 확인, 외부 사이트 검증은 다시 실행하지 않았다. 이번 확인은 업로드 상태에 대한 것이며 브라우저 전체 완성이나 배포 검증을 의미하지 않는다. 새 빌드나 앱 실행, 임시 서버 및 빌드 생성물 추가는 없고 사용자 프로필과 탭 데이터도 변경하지 않았다. 기존 미구현 범위는 제품 계획에 유지한다.

## 2026-10-11, GitHub 비공개 저장소 업로드 요청

사용자가 현재 Naru를 Git에 비공개로 올려 달라고 명시 요청했다. origin은 `https://github.com/FEABE/naru-browser.git`이며 GitHub 조회에서 visibility PRIVATE·isPrivate true·쓰기 권한 ADMIN을 확인했다. 원격 main은 초기 커밋 `0d1ce47a048399f014f6276caa9ddb45ccfc5d41`이고 현재 로컬 브랜치는 `codex/navigation-readiness`다. 기존 개발 브랜치 이름으로 현재 소스·테스트·문서와 로컬 작업 규칙을 업로드하는 범위다.

업로드 대상에서 `.local-data`, `.cache`, `target`, `dist`, `artifacts`, `.env`와 Python 캐시를 제외한다. 사용자 프로필·쿠키·다운로드 파일·앱 번들·로컬 검증 로그를 Git에 넣지 않는다. 공개 범위를 변경하거나 앱을 배포하는 작업은 포함하지 않는다. 이번 요청에서는 제품 코드를 추가 수정하지 않았으며 직전 최종 소스의 Rust 164개·CEF 회귀 32개와 실제 다운로드 검증 결과를 유지한다. 커밋 전 diff 검사와 업로드 대상 점검을 수행하며 원격 업로드 뒤 GitHub 공개 범위 및 원격 커밋 일치를 다시 확인한다. 실제 실행 결과는 로컬 `artifacts/git-private-upload-2026-10-11.json`과 최종 응답에 기록한다.


## 2026-10-11, 다운로드 v4 승인 후 기본 흐름·회귀·정리 완료

사용자 승인 뒤 같은 v4 바이너리의 로컬 HTTP가 complete·committed true로 바뀌었다. 추가 빌드나 키체인 설정 변경은 하지 않았다. 끊긴 응답의 실제 로그에서 다운로드 ID 16의 최초 OnBeforeDownload 한 번과 Downloading 상태의 반복 콜백 다섯 번을 확인했다. v3에서 중복 ID를 최대 4개 오류로 거절하던 원인을 이 실행으로 확정했다. v4에서는 같은 승인 경로로 재시도한 뒤 중단 코드 38·진행 0개를 표시했고, 잘못된 개수 제한 메시지와 부분 파일은 없었다.

동일 v4에서 정상 파일의 완료 상태·131,072바이트·SHA-256 일치를 다시 확인했다. 느린 파일은 27%·2,293,760바이트에서 취소해 진행 0개, 서버 활성 전송 0개와 연결 종료 증가를 확인했다. 부분 파일은 없었고 완료 기록을 지워도 정상 저장한 파일은 유지됐다. v3에서 확인한 파일 입력·AI 인계·beforeunload 사례와 구분해 `artifacts/downloads-v4-validation.json`에 기록했다. v4에서 모든 경합을 다시 실행한 것으로 표현하지 않는다.

기존 CEF 회귀는 select 10개·tasks 6개·spaces 10개·single_window 6개, 총 32개가 통과했다. 각 suite exit 0·stderr 0이며 생성 작업 9개는 finished·빈 tabs/closingTabs였다. 원래 사용자 빈 탭 1과 대기·캡처·생성 예약 0을 보존했다. 첫 실행은 리다이렉션으로 미리 만든 로그 파일 이름이 실행기의 덮어쓰기 방지 조건에 걸려 검사 시작 전에 종료됐다. 해당 로그를 `artifacts/downloads-v4-runner-first-preflight.log`로 보존하고 최초 실제 실행 결과를 `-suite.json`·`-suite-state.json`에 기록했다.

앱을 정상 종료(exit 0)하고 같은 SHA로 재실행했다. 자체 AI 탭의 HTTP는 추가 질문 없이 완료됐으며 작업을 종료해 빈 탭 하나로 돌아왔다. 다운로드 목록은 비었고 최근 닫은 탭 메뉴도 비활성화됐다. 결과는 `artifacts/downloads-v4-restart.json`에 있다.

정리 중 기존 fixture는 SIGTERM 뒤에도 빈 HTTP 연결 때문에 대기했다. Naru를 정상 종료하자 서버도 exit 0으로 종료됐고 강제 종료는 하지 않았다. `tests/download_fixture.py`의 Handler에 3초 timeout을 설정해 요청줄·헤더 이전부터 적용했다. 별도 임시 서버에서 정상 HTTP·파일 해시를 확인하고 빈 연결과 미완성 헤더 연결을 열어 둔 채 SIGTERM을 보내 3.012초·exit 0·빈 stderr로 종료를 확인했다. 첫 제한 환경의 bind 거절과 승인된 소켓 환경의 성공을 `artifacts/downloads-v4-fixture-shutdown.json`에 구분했다. Python fixture 변경은 앱 바이너리를 바꾸지 않는다.

최종 앱은 동일 바이너리로 세션 30322·메인 PID 86688 하나만 실행 중이다. 이전 PID 76784·84289·86111과 자체 fixture PID 76657은 종료했고, 자체 다운로드 파일과 임시 저장 폴더를 정리했다. 사용자 프로필과 원래 빈 탭 1을 보존했다. dist는 최신 앱 하나, target/release는 실행 파일 세 개이며 빌드에서 정리한 4,332개·2.5GiB 외에 새 빌드 중간물을 만들지 않았다. target 2.8MiB·dist 335MiB·검증 자료 13MiB·CEF 캐시 339MiB다.

메모리 표본은 HTTP 확인 뒤 빈 탭 상태의 RSS 합계 최대 681.312MiB, 새 앱·빈 탭 하나에서는 최대 674.438MiB였다. 공유 매핑을 중복 계산하는 합계이고 물리 RAM 사용량이나 최적화 전후 절감량이 아니다. 새 앱 표본에도 renderer 프로세스 두 개가 관찰됐다. 이 값만으로 누수 원인이나 UI renderer 추가를 단정하지 않는다. 메모리 문서에 조건과 원본을 남겼다.

README·다운로드 설계·제품 계획·메모리 문서를 갱신했다. 이번 다운로드 기본 흐름과 기존 회귀 검증은 마쳤지만 연속 합성 다운로드 미접수의 권한 콜백 확인, 모든 작업·탭·앱 종료 경합, 실제 동시 상한, 좁은 창, 장시간 전송, 외부 서비스와 Preseller 내장은 남아 있다. AppKit 초기 행 geometry 경고의 원인도 미확정이다. 전체 Naru 완성으로 표시하지 않았고 목표 도구의 paused 상태를 임의로 변경하지 않았다. 커밋·푸시·배포는 하지 않았다.


## 2026-10-11, 다운로드 v4 재시도 수정과 최종 승인 대기

`src/download_runtime.rs`에서 동일 다운로드 ID의 재시도와 신규 항목을 구분했다. 같은 탭의 Downloading 상태이고 취소 요청이 없으며 기존 저장 콜백을 소비한 경우에만 승인된 경로와 원래 TaskScope를 다시 검사한다. 진행 콜백의 임시 경로 대신 사용자가 선택한 원문 경로를 활성 슬롯에 최대 4개×8,192바이트로 보관하고 종료 시 해제한다. 재시도는 행·슬롯·저장창을 추가하지 않는다. 다른 탭·종료·취소·미승인 상태는 거절하며 이를 최대 4개 오류로 표시하지 않는다. ID·탭·상태만 남기는 진단 로그를 추가했다. v3 중단 응답의 same-ID 재시도 원인 여부는 v4 실제 콜백 로그로 확인할 예정이며 아직 확정하지 않는다.

경계 회귀 2개를 추가한 Rust 전체 164개, 전체 타깃 Cargo 검사, cargo fmt, git diff 검사가 통과했다. 기존 test 전용 dead_code 경고 2개는 남는다. `artifacts/downloads-v4-rust-tests.log`, `-check.log`, `-build.log`에 실행 결과를 보존했다. v4 release SHA는 `e91508161347b45b6de4a32652915c828cf1aef949dffa8f14696bf97d42e471`, CLI는 `af3d25a994fca586145addddddf29daf2a30ebea8df3a68eb380165901afdd7b`다. 빌드에서 4,332개·2.5GiB를 정리했다.

기존 v3를 정상 종료한 뒤 같은 tasks-phase1 프로필로 세션 1425·메인 PID 84289를 실행했다. 원래 빈 사용자 탭 1을 보존하고 자체 fixture 탭 2를 열었으나 5초 wait에서 commit 전 WAIT_TIMEOUT이었다. 앱 시작 20:18:19 직후 새 SecurityAgent PID 84297이 20:18:21에 실행됐다. 새 서명으로 인한 키체인 대기로 의심하고 사용자에게 직접 승인을 요청했다. 보안 창을 도구로 확인하거나 대신 승인하지 않았다.

v4 실제 다운로드와 32개 CEF 회귀·동일 바이너리 재시작은 아직 미실행이다. 준비된 `artifacts/downloads-v4-suite-run.py`는 새 SHA를 고정하며, 승인 뒤 root가 앱과 기존 빈 탭만 남은 상태를 확인한 다음 실행한다. v3의 성공 결과를 v4의 전체 native 통과로 대체하지 않는다. 현재 결과와 프로세스·서버 핸들은 `artifacts/downloads-v4-validation.json`과 `-cli.jsonl`에 있다.

정리 확인에서 dist는 최신 앱 하나, target/release는 최신 실행 파일 세 개만 남았다. target 2.8MiB·dist 335MiB·검증 자료 13MiB·CEF 캐시 339MiB다. v3의 자체 128KiB 파일은 해시와 파일 입력 검증 뒤 삭제했다. 사용자 프로필과 원래 탭을 유지하며 승인 뒤 검증을 위해 최신 앱, 자체 fixture PID 76657, 자체 탭 2와 빈 검증 저장 폴더만 유지한다. README·제품 계획·다운로드 설계·메모리 문서를 갱신했다. 새 RSS·외부 사이트·Preseller 내장은 미검증이며 커밋·푸시·배포는 하지 않았다. 목표 도구 조회는 paused를 반환했고 상태를 임의로 바꾸지 않았다.


## 2026-10-11, 다운로드 v3 실제 저장·취소·제어권 검증과 중단 응답 결함

두 FFI null 인수를 수정한 release SHA `83522cdfce1f959f6b0ec3bf7baa276f08e193618b9e82c035fd61477a58ae89`를 같은 프로필로 실행했다. 세션 97963·PID 76784에서 로컬 HTTP가 완료됐고 별도 키체인 질문은 하지 않았다. 실제 네이티브 저장창의 Save 뒤 131,072바이트 파일과 SHA-256 `59f410ae5e17962412e2aed4f815918f634932f2abf084f00bb638c4db017850`가 fixture 원본과 일치했다. 저장창 Cancel, 느린 파일의 28% 진행 표시와 취소, 진행 0개 복귀를 확인했다. 서버는 연결 종료 1회를 기록했고 취소한 느린 파일이나 부분 파일은 남지 않았다. 완료 기록 지우기 뒤에도 저장 파일은 유지됐으며 같은 파일을 native file input에서 선택했을 때 이름·크기·trusted true가 확인됐다. 외부 업로드는 없는 로컬 fixture다.

첫 사용자 다운로드 뒤 두 번째 native 클릭도 저장 대기에 도달했다. 클릭 직후 AX에 개수가 없었던 관찰만으로 차단됐다고 말한 판단은 철회한다. 이전 v2의 합성 반복 클릭 미접수는 별도 조건이며 연속 다운로드 권한 콜백 진입은 아직 확인하지 않았다. 권한 자동 허용은 추가하지 않았다.

자체 AI 작업의 다운로드 대기는 사용자 공간·탭 2를 유지했다. 저장창을 열고 CLI handoff로 작업을 중지한 뒤 다른 파일 이름으로 Save를 눌러도 파일은 만들어지지 않고 취소 상태가 됐다. 작업을 종료해 자체 탭 3을 정리했다. 사용자 탭의 beforeunload에서 Cancel은 탭과 대기 다운로드 1개를 유지했고, 두 번째 닫기의 OK는 실제 탭 종료와 다운로드 취소·진행 0개를 만들었다. 원래 사용자 빈 탭 1을 남겼다.

중간 종료 응답은 새 문서의 첫 CLI 클릭으로 대기 1개를 만든 뒤 Save에서 '취소됨'과 잘못된 최대 4개 메시지를 표시했다. 동일 ID 콜백을 상한 초과로 처리하는 코드가 있어 재시도 흐름을 조사·수정 중이다. 해당 사례를 성공이나 일반적인 네트워크 중단 처리로 세지 않는다. AppKit 초기 행 geometry 경고도 원인은 확정하지 않았고 화면 검사에서 직접적인 실패는 확인하지 않았다.

결과는 `artifacts/downloads-v3-validation.json`과 `-cli.jsonl`에 기록했다. v3 빌드에서 1,263개·670.8MiB를 정리했고 새 바이너리는 native Quit 뒤 세션 exit 0으로 종료했다. 32개 CEF 회귀는 수정 뒤 최종 바이너리에서 실행할 예정이며 v3에서 실행하지 않았다. 기존 Rust 162개는 두 FFI 수정 전 v1 결과다. 자체 fixture 서버만 다음 검사에 재사용 중이고 사용자 데이터는 보존했다. 전체 브라우저 완성으로 보고하지 않는다.


## 2026-10-11, 다운로드 v2 저장창 열기 확인과 경로 출력 인자 수정

v1의 빈 필터를 유효한 CefStringList로 바꾼 v2 release SHA는 `80cfbc29c28a5f6f8ce1436df6e96ec0e43f9449180d804f2288a7d347b2f239`다. 빌드에서 중간 파일 1,263개, 670.8MiB를 정리했다. 앱 세션 24357, PID 72774의 첫 로컬 HTTP는 load complete였다. v2에서는 별도 승인 질문을 만들지 않았다. 실제 저장창이 열렸으며 Cancel을 누르면 취소됨과 진행 0개로 돌아갔다.

그 뒤 같은 문서의 두 번째 합성 클릭과 후속 native 클릭은 서버 요청이나 새 행을 만들지 않았다. 새로 고친 문서의 첫 native 클릭은 다시 요청을 접수했다. 설치 CEF 156의 MULTIPLE_DOWNLOADS 권한과 Alloy의 기본 IGNORE 계약, 현재 permission_handler 누락을 확인했다. 이 경로가 실제 반복 실패 원인인지는 해당 콜백을 관찰하지 않아 아직 확정하지 않았다. 다른 사이트나 공간에 영향을 주는 전역 자동 다운로드 허용은 적용하지 않았다.

새 요청의 저장창에서 workspace 검증 폴더를 선택하고 Save를 누르자 exit 139로 다시 종료됐다. 폴더는 비어 있으며 파일 저장 성공으로 세지 않는다. 같은 바이너리를 LLDB로 재현해 cef_string_utf16_set의 EXC_BAD_ACCESS address 0x10, 출력 포인터 x2=0을 확인했다. SaveDialog의 CefString::default()는 쓰기 가능한 빈 구조체가 아니라 null borrowed 문자열이었다. CefString::from(빈 문자열)로 초기화하고 null을 검사하도록 수정했다. 첫 빈 필터 수정은 유지하며 새 코드의 mutable FFI 인수를 다시 검토했다. Rust 모델과 수명 검사 162개는 v1 소스 결과이고 이 FFI 수정 뒤 재실행하지 않았다. 실제 저장 재검증을 위해 v3를 빌드 중이다.

원본 CLI는 `artifacts/downloads-v2-cli.jsonl`, v2 상태는 `-validation.json`, 두 번째 충돌 스택은 `-debug-crash.log`, 서버 요청 수는 `-fixture-after-repeat.json`이다. 오류 뒤 CUA가 연 추가 앱은 native 메뉴로 종료했고 예외로 정지한 자체 디버거 PID 74385는 process kill, LLDB 세션 33510은 exit 0으로 정리했다. 기존 프로필과 원래 사용자 탭을 대체하는 빈 탭 1은 다음 같은 프로필 실행에서 유지한다. 실제 저장, 진행 중 취소, 작업과 탭 수명, CEF 32개 회귀는 아직 미완료다.

## 2026-10-11, 다운로드 v1 구현과 실제 저장창 종료 결함 추적

다운로드를 받을 DownloadHandler가 없던 경로에 제한된 목록, 저장 위치 선택, 진행 상태와 취소를 연결했다. `src/downloads.rs`는 대기와 진행, 취소 확인 대기를 합쳐 4개, 완료 기록까지 합쳐 24개를 유지한다. `src/download_runtime.rs`는 작업 ID와 revision 검증, 명시적 네이티브 저장창, 저장창 응답의 세대 확인과 CEF 콜백 수명을 관리한다. `src/browser.rs`의 실제 닫기 승인과 작업 해제에 정리를 연결했고 `src/control_panel.rs`에 같은 창의 다운로드 화면과 사이드바 개수를 추가했다. 목록을 닫으면 행 UI를 해제하며 별도 DB, 상주 worker, UI renderer를 추가하지 않았다. 설계는 `docs/downloads-design.md`에 기록했다.

순수 모델 8개, 최종 Rust 전체 162개, 전체 타깃 Cargo 검사, cargo fmt와 git diff 검사가 통과했다. 검사 로그는 `artifacts/downloads-v1-model-tests.log`, `-rust-tests.log`, `-check.log`, `-fmt.log`다. Rust 2024 옵션 없이 직접 rustfmt를 호출한 첫 확인은 문법 오류를 반환했고 edition 2024로 바로잡은 검사와 cargo fmt는 통과했다. 전체 타깃 검사의 기존 test 전용 dead_code 경고 2개는 남는다.

이전 shortcut v1 앱은 네이티브 메뉴로 정상 종료했고 세션 39375 exit 0을 확인했다. 새 v1 release SHA는 `739f1289bba5e8b8017288df3ca7e0402d5d51b4b1f1a5fd969d34621c2dd54d`다. 빌드에서 중간 파일 4,847개, 2.5GiB를 정리했다. 앱 세션 77312, 메인 PID 68647을 기존 tasks-phase1 프로필로 실행했다. 자체 HTTP 탭 2의 첫 wait는 commit 전 WAIT_TIMEOUT이었다. SecurityAgent 실행을 확인하고 사용자에게 직접 승인을 요청했으며, 승인 후 같은 바이너리의 로컬 페이지가 열렸다.

네이티브 UI에서 빈 다운로드 목록, 사이드바와 보기 메뉴로 열기, Escape 복귀, 주소 초안 보존을 확인했다. 작은 파일 링크를 직접 클릭하면 저장창을 자동으로 열지 않고 대기 1개 표시와 파일 행을 만들었다. 그러나 행의 저장 위치 선택을 누르자 프로세스가 exit 139로 종료됐다. 따라서 실제 저장과 취소, 진행 표시는 통과한 기능으로 기록하지 않는다. CUA 후속 상태 조회가 동일 bundle 앱을 PID 69705로 다시 열었으며, 이 프로세스는 네이티브 메뉴로 종료했다. 이 자동 재실행은 기본 Application Support 프로필을 사용한다. 그 프로필은 조회하거나 삭제하지 않았다.

같은 v1 바이너리를 LLDB로 기존 tasks-phase1 프로필에서 재현했다. 자체 HTTP는 추가 키체인 승인 없이 열렸다. 저장 위치 선택에서 EXC_BAD_ACCESS address 0x8이 발생했고 첫 프레임은 cef_string_list_size, x0 레지스터는 0이었다. 스택은 Naru choose_path에서 CEF 목록 변환으로 이어졌다. Rust bridge는 accept_filters=None을 null로 넘기는 반면 설치된 공식 C++ bridge는 필터가 없어도 빈 목록을 할당한다. 이 차이를 원인으로 확인했고 유효한 빈 목록 전달로 수정 중이다. 원본 스택은 `artifacts/downloads-v1-debug-crash.log`, 첫 실행 응답은 `-setup.json`, 재현 준비는 `-debug-setup.json`, 종합 상태는 `-validation.json`에 있다. 예외로 정지한 자체 PID 70188은 debugger process kill로 정리했고 LLDB 세션 92787도 exit 0으로 종료했다.

`tests/download_fixture.py`와 고정 HTML은 작은 파일의 해시, 느린 응답, 중간 종료, 파일 입력과 닫기 확인용 로컬 fixture다. 독립 HTTP 검사는 성공했으나 CEF 파일 저장 성공과 구분한다. 실제 파일 저장, 저장창 취소, 작업과 탭 종료 경합, 기본 파일 입력, 동일 최종 바이너리의 CEF 32개 회귀는 아직 미실행이다. 새 RSS나 외부 사이트, Preseller 내장도 확인하지 않았다. 커밋, 푸시, 배포는 하지 않았다. 전체 목표는 active이며 수정 후 검증을 계속한다.

## 2026-10-11, shortcut v1 승인 후 실제 단축키와 직접 입력 확인

사용자 승인 뒤 앱 세션 20112가 계속 실행 중임을 확인했고, 같은 SHA `e77a65a5c04efe8e596de1783d841698c34424476915622ef04e9ee0f64b22a1`의 탭 2가 HTTP complete·committed true·epoch 3으로 로드됐다. 추가 빌드나 서명 변경은 하지 않았다. 첫 CUA 캡처는 본문이 비어 보였지만 탭을 눌러 활성화한 뒤 fixture 본문이 표시됐다.

검증 탭을 자체 AI 작업에 연결했다. native SELECT를 직접 클릭하면 paused revision 2가 됐고, 메뉴를 닫고 revision 3으로 재개한 뒤 ⌘⇧A를 누르면 검색을 열면서 active·revision 3을 유지했다. Escape로 복귀한 뒤 ⌘⇧B의 사이드바 접기·펼치기와 ⌘⇧S의 공간 보기에서도 active·revision 3이 유지됐다. 앞선 select v1에서 같은 작업을 중지하던 결함이 이번 조건에서 재현되지 않았다.

같은 자체 탭을 로컬 사용자 입력 fixture로 이동했다. 입력란 직접 클릭은 paused revision 4를 만들었다. revision 5로 재개한 뒤 CUA native paste는 다시 paused revision 6을 만들었고, 재개 후 read에서 정확한 문자열과 trusted input 1회를 확인했다. 단축키 예외 때문에 일반 붙여넣기까지 handoff를 건너뛰는 회귀는 이번 조건에서 없었다. 실제 IME 조합 검증은 아니다.

자체 작업은 finished revision 8·빈 tabs/closingTabs로 끝냈고 반환된 탭 2를 닫아 원래 빈 사용자 탭 1을 복원했다. 대기·캡처·생성 예약은 모두 0이었다. 원본 응답과 CUA 관찰은 `artifacts/shortcut-v1-native.json`에 기록했다. 이어 CLI 독점 실행으로 준비된 32개 회귀 검사를 시작했다. 최종 결과는 select 10개·tasks 6개·spaces 10개·single_window 6개, 총 32개 통과다. 각 suite와 실행기 exit 0·stderr 0이었고, 생성 작업 9개는 모두 finished·빈 tabs/closingTabs였다. 원래 user 탭 1과 대기·캡처·생성 예약 0을 확인했다. `artifacts/shortcut-v1-suite.json`과 `-state.json`에 동일 바이너리 결과를 저장했다.

검증용 최근 닫은 탭 기록을 비우기 위해 같은 앱을 정상 종료·재실행했다. 종료 직후 CUA 상태 조회는 timeout이었지만 앱 세션 20112의 exit 0을 별도로 확인했다. 추가 빌드 없이 같은 SHA로 세션 39375·메인 PID 55827을 실행했고, 자체 AI 탭의 로컬 HTTP는 추가 승인 없이 완료됐다. 작업은 finished revision 2·빈 tabs/closingTabs로 종료했다. 파일 메뉴의 최근 닫은 탭은 비활성화됐으며 원래 빈 사용자 탭 1을 유지했다. 자체 fixture PID 41085는 명령을 확인한 뒤 SIGTERM으로 종료하고 세션 86312 exit 0을 확인했다.

최종 프로세스·파일 조회에서 최신 메인 PID 55827 하나, `dist/Light Browser.app` 하나, release 실행 파일 세 개만 남았다. 이전 앱 PID 41422·53449와 서버 PID 41085는 없었다. 앞선 빌드에서 정리한 4,332개·2.4GiB 외에 새 컴파일 중간 파일을 만들지 않았다. 현재 상태는 `artifacts/shortcut-v1-validation.json`, 재시작은 `-restart.json`, 정리는 `-cleanup.json`에 기록했다. README·제품 계획·선택 계약·메모리 문서를 갱신했다.

이번 폼 선택·단축키 수정의 로컬 검증과 정리를 마쳤다. 실제 IME 조합·새 RSS·외부 사이트·장시간 사용·Preseller 내장은 검증하지 않았으며 브라우저 전체 완성을 선언하지 않는다. 목표 도구는 이전 승인 대기의 blocked 상태를 반환했고 도구에는 active로 바꾸는 동작이 없었다. 실제 승인 의존은 해소됐으므로 새 blocked나 완료 상태를 설정하지 않았다. 커밋·푸시·배포는 하지 않았다.

## 2026-10-11, 공통 Command 단축키 수정 release와 최종 검증 대기

폼 선택을 검증하던 select v1에서 실제 재현된 ⌘⇧A·⌘⇧B의 잘못된 AI 작업 중지를 수정했다. `src/control_panel.rs`의 reopen·일반 단축키·메뉴 예외가 같은 Command 키 해석을 사용한다. 일반 입력·Control/Option·CtrlTab·CmdV handoff 경로는 유지한다. 순수 회귀 3개를 추가했고 Rust 151개와 전체 타깃 검사를 통과했다. 수정 앱의 실제 단축키 검증은 아직 하지 않았다.

이전 앱은 네이티브 메뉴로 정상 종료한 뒤 세션 74620 exit 0을 확인했다. `artifacts/shortcut-v1-build.log`에서 release·서명·번들 교체와 Cargo 중간 파일 4,332개·2.4GiB 정리를 확인했다. 새 앱 SHA-256은 `e77a65a5c04efe8e596de1783d841698c34424476915622ef04e9ee0f64b22a1`이며 기존 프로필로 세션 20112를 실행했다. 원래 빈 사용자 탭 1을 보존한 채 만든 검증 탭 2는 첫 5초 load에서 commit 전 WAIT_TIMEOUT이었다. 새 SecurityAgent PID 53469를 확인하고 사용자에게 직접 키체인 승인을 요청했다. 앞선 사용자의 승인은 select v1에서 해소됐으며 이번 요청은 코드가 바뀐 새 바이너리에 대한 것이다.

`artifacts/shortcut-v1-suite-run.py`는 새 SHA와 새 결과 경로를 고정하고 구문·설정만 검사했다. 실제 실행하지 않았다. 승인 뒤 같은 바이너리의 SELECT 포커스에서 ⌘⇧A/B/S와 일반 본문 입력 제어권을 검사하고, 검증 탭을 정리한 다음 선택 10개·기존 회귀 22개를 순차 실행한다. 첫 select v1 실패·독립 포커스 진단·수정 후 같은 v1의 32개 통과는 그대로 보존했다. 새 빌드 성공으로 이전 바이너리의 검사 결과를 대체하지 않는다. 실행 프로세스 조회에서는 최신 메인 PID 53449 하나를 확인했다. target 약 2.7MiB, dist 약 335MiB, 검증 자료 약 12MiB, CEF 캐시 약 339MiB다. 승인 후 같은 요청을 이어가기 위해 직접 띄운 fixture PID 41085와 검증 탭 2만 유지했다.

현재 상태·핸들·후속 절차는 `artifacts/shortcut-v1-validation.json`에 남겼다. README·제품 계획·선택 계약·메모리 문서도 갱신했다. 새 RSS·외부 사이트·실제 IME 조합·Preseller 내장 검사는 하지 않았고 커밋·푸시·배포도 하지 않았다. 전체 브라우저 목표는 미완료다.

## 2026-10-11, select v1 승인 후 실제 선택 검증과 발견한 결함

사용자의 ‘항상 허용’ 응답 뒤 앱 세션 74620과 fixture 세션 86312를 직접 조회해 실행 중임을 확인했다. 동일 SHA-256 8a7e81db40966fecbe982661cb78fc6a0a6ffdb38dfb5ebf450117dc123548e8의 탭 2가 HTTP complete·committed true·provisional false로 바뀌었다. 승인 의존은 해소됐다. 목표 도구 조회는 기존 blocked 상태를 반환했으나 승인 응답에 따라 실제 작업을 재개했다. 목표 완료로 표시하지 않았다.

실제 CLI Alpha→Beta 뒤 CUA 화면에서 Beta를 확인했다. AI 작업에 테스트 탭을 연결한 뒤 공백 포함 값을 선택하면 Exact spaces 표시와 정확값·합성 input/change 순서를 유지했다. 같은 값 요청은 changed false였고 이벤트가 추가되지 않았으며 active·revision 1이 유지됐다. native SELECT를 직접 클릭하면 paused·revision 2가 됐고 Alpha 선택은 trusted input/change를 만들었다. 최초 QA 스크립트가 paused 작업에 read를 시도해 TASK_PAUSED를 받았으며, 이는 정상 보호다. revision 3으로 재개한 뒤 실제 Alpha 결과를 읽었다.

같은 native SELECT에 포커스가 있는 상태에서 ⌘⇧A는 검색을 열지만 active revision 3을 paused revision 4로 바꾸었다. Escape로 닫은 뒤 revision 5로 재개하고 ⌘⇧B를 보내면 사이드바를 접으면서 paused revision 6이 됐다. 따라서 공통 단축키의 조건부 소스 위험이 실제로 재현됐다. 정확한 AppKit 원본 문자는 기록하지 않았으며 IME 조합 성공·실패의 증거는 아니다. 작업을 finished revision 7로 끝내고 반환된 테스트 탭 2를 닫아 원래 사용자 탭 1만 남겼다. 원본은 artifacts/select-v1-native.json이다.

고정 SHA 실행기는 최초 1회 실행에서 선택 메타데이터·일반 선택/무동작·빈값/공백/UTF-16 경계·거절 조건·숨긴 대상 재검사의 기능 5개를 통과했다. focus 중 요소 교체 검사에서 ELEMENT_UNAVAILABLE 대신 changed true가 반환돼 종료했다. 정리 검사까지 합한 보고서 checks는 6개이며 전체 10개 통과가 아니다. 후속 기존 22개는 실행하지 않았다. 원래 탭 1과 user 선택, 생성 작업 2개의 finished·빈 tabs/closingTabs, 대기·캡처·생성 예약 0과 자체 서버 스레드 종료를 확인했다. 실패 원본은 artifacts/select-v1-suite-select.stdout.json, 정리는 artifacts/select-v1-suite-state.json에 보존했다.

배경 AI 탭은 native focus를 거절하고 CEF 뷰를 숨긴다. 기존 fixture는 focus 이벤트에서만 노드를 교체하지만 focus가 실행됐는지 기록하지 않았다. 따라서 자동 검사 실패를 즉시 제품 결함으로 단정하지 않고 배경 선택 전후·표시 후·새 foreground 문서를 나눠 진단 중이다. 제품 DOM 코드는 바꾸지 않았다. 별도 src/control_panel.rs는 Command-aware 키 해석을 reopen·일반 단축키·메뉴 예외에 공통 적용하도록 수정했다. Control/Option·일반 입력·CtrlTab·CmdV 처리와 메뉴 전달을 유지하며 순수 회귀 3개를 추가했다. 단위 검사와 실제 수정 앱 검증은 후속 기록으로 갱신한다.

독립 진단에서 배경 탭은 hasFocus false·hidden이고 activeElement만 바뀌며 focus 횟수와 요소 교체는 0이었다. 공간 표시 뒤 hasFocus true·visible과 focus/교체 각 1이 됐다. 새 전경 문서에서는 SELECT·OPTION 교체를 모두 ELEMENT_UNAVAILABLE로 거절하고 input/change 0·선택 0을 유지했다. 최초 검사의 배경 focus 가정이 틀렸음을 확인했으며 제품 DOM 코드는 유지했다. 자체 작업·탭·서버를 정리하고 원래 user 탭 1을 복원했다. 근거는 artifacts/select-v1-focus-diagnostic.json이다.

공통 단축키 수정의 Rust 검사는 제한 환경에서 148개 통과·소켓 권한 3개 실패였고, 로컬 소켓 권한을 허용한 재실행은 151개 모두 통과했다. 전체 타깃 cargo check도 성공했다. 근거는 artifacts/shortcut-v1-rust-tests-local.log와 artifacts/shortcut-v1-check.log다. 현재 실행 앱에는 아직 이 단축키 수정이 포함되지 않았다.


수정한 focus 검사는 사용자 공간을 보존하는 배경 검사를 그대로 유지하고, 교체를 검사할 때만 해당 공간을 표시한 새 문서의 hasFocus·visible을 확인한다. SELECT·OPTION 각각 교체 1회·input/change 0·선택 불변과 새 ref의 성공을 확인한 뒤 finally에서 user로 복귀한다. 동일 select v1에서 select 10개·tasks 6개·spaces 10개·single_window 6개, 총 32개가 모두 exit 0·stderr 0으로 통과했다. 생성한 작업 9개는 모두 finished·빈 tabs/closingTabs이며 원래 빈 사용자 탭 1과 대기·캡처·생성 예약 0을 보존했다. 최초 실패 자료는 유지하고 artifacts/select-v1-corrected-suite.json과 -state.json에 수정 후 결과를 따로 저장했다. 네이티브 앱 종료 메뉴 뒤 세션 74620의 exit 0을 확인하고 공통 단축키 수정의 shortcut v1 release 빌드를 시작했다.

## 2026-10-11, select v1 release 반영과 새 바이너리 검증 준비

이전 턴은 재열기 v2 실제 수정 확인·CEF 22개 회귀·동일 바이너리 재시작과 select 소스 구현으로 진행한 작업이었다. 이번 턴에는 기존 앱 세션 50870을 조회해 실행 중임을 확인했고 CLI 상태에서 원래 사용자 탭 1, user 공간과 대기·캡처·생성 예약 0을 확인했다. 네이티브 종료 메뉴 뒤 exit 0을 확인하고 새 빌드를 만들었다.

`artifacts/select-v1-build.log`의 release 빌드·서명·번들 교체가 성공했다. Cargo 중간 파일 1,263개·670.0MiB를 정리하고 최신 앱 하나와 release 실행 파일만 남겼다. SHA-256은 `8a7e81db40966fecbe982661cb78fc6a0a6ffdb38dfb5ebf450117dc123548e8`다. 새 release CLI help에 select가 포함됐음을 확인했다. Rust 148개·모의 DOM 37개는 앞선 소스 검사 결과이며 이번에 다시 실행한 것은 아니다.

`tests/preview_fixture.py`에 고정 `/select.html` 경로만 추가했다. Python 구문과 실제 로컬 HTTP 200·UTF-8·fixture 본문 일치를 확인했다. 서버는 세션 86312·PID 41085, 주소는 http://127.0.0.1:53294다. 새 앱은 원본 프로필로 세션 74620·PID 41422 하나를 실행했다. 직접 만든 탭 2의 5초 load는 WAIT_TIMEOUT·commit false·documentAvailable false·provisional true였다. 새 SecurityAgent PID 41441을 확인해 사용자의 직접 키체인 승인을 요청했다. 도구로 보안 창을 조작하지 않았다. 승인 전이므로 선택 기능이 실제 CEF에서 성공했다고 표시하지 않는다.

별도 검토는 기존 Command-aware 해석이 ⌘⇧T에만 적용되고 일반 단축키·메뉴 예외는 여전히 charactersIgnoringModifiers를 사용하는 조건부 경로를 확인했다. 비라틴 문자가 반환될 경우 조기 단축키 분류가 실패해 본문 handoff 후 메뉴 전달이 가능하다. 현재 입력원에서 ⌘⇧A/B/S의 실제 재현 여부는 확인하지 않았고 이 턴에 제품 코드를 수정하지 않았다. 승인 뒤 실제 페이지와 AI revision으로 확인할 항목이다.

승인 후에는 같은 바이너리에서 native 선택값·receipt를 확인한 뒤 검증용 탭 2를 정리하고 원래 탭 1만 남겨 select 10개와 기존 CEF 22개를 순차 실행한다. 실행하지 않은 검사는 성공으로 세지 않는다. 원본 응답은 `artifacts/select-v1-native.json`, 빌드·현재 핸들·다음 절차는 `artifacts/select-v1-validation.json`에 있다. README·제품 계획·선택 계약·메모리 문서를 갱신했다. target 약 2.7MiB·dist 약 335MiB·artifacts 약 12MiB이며 프로필을 보존했다. 새 RSS·실제 IME·외부 사이트·Preseller 내장은 검사하지 않았다. 커밋·푸시·배포는 하지 않았고 전체 목표는 active다.


실행기 `artifacts/select-v1-suite-run.py`를 작성했다. 앱·CLI SHA를 고정하고 기존 실행 자료 덮어쓰기를 거절하며 select 10개와 선택한 후속 22개를 순차 실행한다. 실패·정리 실패 시 다음 검사를 시작하지 않고 원래 사용자 탭 1·검증 작업·대기 자원을 확인한다. Python 구문·정적 교차 검토를 마쳤지만 실제 실행은 아직 하지 않았다. 동일 탭의 후속 2초 load도 commit 전 WAIT_TIMEOUT이었고 자원 대기 수는 모두 0이었다. 바이너리 SHA는 같으며 추가 빌드·재시작은 하지 않았다.

다음 자동 재개에서 앱 세션 74620과 fixture 세션 86312를 직접 조회해 둘 다 실행 중임을 확인했다. 같은 SHA의 탭 2는 2초 대기 후에도 commit 전 WAIT_TIMEOUT이었다. 이번 턴은 실행 핸들을 확인한 승인 대기이며, 제품 코드·바이너리를 바꾸거나 검사 실행기를 시작하지 않았다. 같은 질문을 다시 만들지 않았고 전체 목표는 active다.

세 번째 연속 goal 턴에서도 앱 세션 74620과 fixture 세션 86312가 실행 중이며 같은 SHA의 탭 2는 commit 전 WAIT_TIMEOUT이었다. 승인에 의존하는 실제 CEF·네이티브 검증으로 진행할 수 없어 목표 도구가 blocked를 반환했다. 완료를 뜻하지 않는다. 같은 바이너리·프로필·서버를 보존했고 새 승인 질문이나 재빌드를 만들지 않았다. 사용자의 승인 응답 뒤 HTTP 준비, select 실제 동작·AI 제어권, 32개 검사와 검증 자원 정리를 이어야 한다.

## 2026-10-11, 재열기 v2 승인 해소·실제 수정 검증·회귀 완료

대기 중이던 탭 2의 로컬 HTTP가 complete·committed true·provisional false로 바뀌었다. 앱 SHA-256 `84bfaf36d80d6b6048461b07fcbef5c5bca2862c3408aa7d2e772e1fa5204100`를 유지한 채 검증했다. 최초 CUA 스크린샷의 본문은 비어 보였지만 탭을 클릭해 활성화한 뒤 실제 문서가 표시됐다. 그 원인은 확정하지 않았다.

실제 사용자 입력란에서 빈 복원 목록의 ⌘⇧T는 DOM keydown·keyup을 만들지 않았다. AI 작업 task-bba5cad5433978f466e7dce9a1626830은 기록이 없을 때와 닫힌 사용자 URL 기록이 있을 때 모두 active·revision 1을 유지했다. 사용자 공간에서 B4 다음 A3을 닫고 단축키는 A5, 파일 메뉴는 B6을 한 번씩 새 사용자 탭으로 만들었다. task 소유권은 없었다. A의 needle 1/8 찾기는 B 복원 때 닫혔고 ⌘G로 복구되지 않았다.

AI 입력란의 직접 클릭은 정상적으로 paused·revision 2를 만들었다. CLI 재개 후 클릭 없이 ⌘⇧T를 보냈으며 active·revision 3과 DOM keydown·keyup 0을 유지했다. v1의 실제 단축키 결함은 이번 조건에서 재현되지 않았다. IME 조합 성공을 뜻하지는 않는다. 원본과 수기 관찰은 `artifacts/reopen-v2-native.json`에 있다. v1의 beforeunload·AI 탭 제외·overlay 보호 전체를 v2에서 반복한 것은 아니다.

자체 작업을 finished·revision 4로 끝내고 반환된 진단 탭 2, 복원 탭 5·6을 닫아 원래 사용자 탭 1만 유지했다. 이후 CLI 독점 실행으로 tasks 6개·spaces 10개·single_window 6개, 총 22개가 통과했다. 각 exit 0·stderr 0바이트였고 생성 작업 7개 모두 finished와 빈 tabs·closingTabs였다. 원래 빈 사용자 탭 1, user 공간과 대기·캡처·생성 예약 0을 확인했다. `artifacts/reopen-v2-suite.json`, `artifacts/reopen-v2-suite-state.json`에 근거가 있다.

네이티브 종료 메뉴 뒤 이전 앱 세션 87916의 exit 0을 확인했다. 같은 바이너리·프로필로 재실행한 세션은 50870, 메인 PID는 38546이다. 파일 메뉴의 최근 닫은 탭은 비활성화됐으며 ⌘⇧T도 기존 빈 탭과 주소 포커스를 유지했다. 별도 검증 작업으로 HTTP를 다시 로드하고 정리해 빈 복원 기록을 유지했다. 작업은 finished와 빈 소유 탭 상태로 정리됐으며 원래 사용자 탭 1을 보존했다. 근거는 `artifacts/reopen-v2-restart.json`이다. 직접 띄운 fixture PID 25653은 SIGTERM 후 세션 44316 exit 0을 확인했다. 이전 메인 PID 30914는 없고 현재 앱 하나만 남았다.

현재 release는 검증한 재열기 v2이며 select는 소스 검사만 완료한 상태다. select 검사 뒤 Cargo 중간 파일 1.9GiB를 제거했고 앱·CLI SHA가 보존됐다. target 약 2.7MiB·dist 약 335MiB·artifacts 약 11MiB다. README·제품 계획·재열기 설계와 종합 검증 기록을 갱신했다. 새 RSS·외부 사이트·실제 IME·최소 창·장시간 사용·Preseller 내장은 검사하지 않았다. 커밋·푸시·배포는 하지 않았고 전체 목표는 계속 active다.


## 2026-10-11, CLI HTML 단일 선택 구현과 소스 검증

웹 폼 제어의 누락을 보완해 `select TAB REF VALUE`와 작업 범위 명령을 추가했다. CLI → 원본 요청 검증 → 작업 소유권·revision → 탭 준비·epoch → DOM action → 응답의 기존 경로를 사용한다. 빈 값과 앞뒤 공백을 그대로 보존하며 값 상한은 UTF-16 코드 단위 1,024개다. CLI는 INVALID_ARGUMENTS, 원본 소켓은 INVALID_REQUEST로 입력 상한을 거절한다. 읽기·요소 세대와 탐색 세대가 다른 ref, 중지되거나 낡은 revision의 작업도 기존 보호를 따른다.

기본 단일 SELECT만 지원한다. 정확한 값의 옵션이 없거나 중복되거나 숨김·비활성이면 변경하지 않는다. focus 뒤 SELECT와 옵션의 노드·상태를 다시 확인해 페이지 처리기의 교체를 거절한다. 실제 변경은 native selectedIndex setter 뒤 합성 input·change 순서로 알리며 이미 선택된 같은 옵션은 포커스나 이벤트를 만들지 않는다. 페이지 처리기의 최종 결과까지 보장하는 계약은 아니다.

옵션 읽기는 SELECT당 40개, read 전체 240개다. 숨김 옵션은 label·value를 생략하고 너무 긴 값도 생략을 명시한다. 표시 label은 240자이며 선택 검사는 옵션 500개 이하로 제한한다. 전체 옵션 배열 복제, 옵션별 ref, 저장 파일·상주 worker·renderer를 추가하지 않았다. 실제 Chromium RAM 절감 측정은 아니다.

Rust 148개와 전체 타깃 검사가 통과했다. 최초 두 실패 중 하나는 task raw 요청의 명령 허용 목록 누락으로 수정했고, 하나는 기존 CLI ref 파서가 줄바꿈을 거절한다는 잘못된 테스트 가정으로 제거했다. 초기 실패 로그를 유지했다. 모의 DOM은 기존 20개와 새 경계 17개를 합한 37개가 통과했다. 실제 CEF용 fixture와 smoke를 작성했고 Python 구문·HTML 구조·격리 HTTP를 확인했다. 정리 경로는 작업의 finished와 빈 tabs·closingTabs를 함께 기다리고 자체 서버 스레드 종료까지 확인한다. CEF smoke 자체는 아직 실행하지 않았다.

재열기 v2 네이티브 검증용 앱·release CLI는 바꾸지 않았으므로 현재 바이너리에 select 명령은 없다. Cargo 중간 파일 3,589개·1.9GiB만 정리한 뒤 앱·CLI SHA가 전과 같음을 확인했다. target 약 2.7MiB·dist 약 335MiB·artifacts 약 11MiB다. 원본 프로필과 실행 중 앱을 보존했다. 계약은 `docs/select-control.md`, 종합 근거는 `artifacts/select-v1-validation.json`이다. README·제품 계획·메모리 문서를 갱신했다. 커밋·푸시·배포는 하지 않았고 전체 목표는 active다.


## 2026-10-11, 수정 바이너리 유지와 macOS 키 재번역 객체 검사

이전 턴은 실제 결함 재현·소스 수정·검사·새 빌드까지 진행한 작업이었다. 이번에는 앱 세션 87916과 fixture 세션 44316을 직접 조회해 모두 실행 중임을 확인했다. 같은 HTTP 탭 2의 3초 load는 commit 전 WAIT_TIMEOUT이었으며 바이너리 SHA는 `84bfaf36d80d6b6048461b07fcbef5c5bca2862c3408aa7d2e772e1fa5204100`으로 같았다. 재시작이나 추가 빌드는 하지 않았다.

승인 대기와 독립적인 검사로 `artifacts/reopen-v2-key-translation.swift`에서 NSEvent 객체를 만들고 Command+Shift 재번역을 확인했다. OS에 이벤트를 보내거나 창을 활성화하거나 입력 소스를 바꾸지 않았다. SDK가 정의한 ANSI T 키코드와 unmodified 문자 t·T·ㅆ의 세 조건에서 재번역은 모두 T로 인식됐다. 기존 unmodified 문자열 판정은 ㅆ 조건만 인식하지 못했다. 이는 새 API가 해당 객체 조건을 처리한다는 증거이며 v1의 실제 이벤트가 ㅆ이었다거나 CEF 전달·AI revision 문제가 해결됐다는 증거는 아니다.

최초 sandbox 실행은 입력 서비스 XPC Connection invalid 경고와 결과를 함께 냈다. 원본을 `artifacts/reopen-v2-key-translation.sandbox.json`과 `.sandbox.stderr.log`에 보존한 뒤, 이벤트를 전송하지 않는 동일 객체 검사를 로컬 실행 권한으로 다시 실행했다. 최종 `artifacts/reopen-v2-key-translation.json`의 세 조건이 같았고 stderr는 비었으며 exit 0이었다. 별도 검사에 사용한 Swift 모듈 캐시 약 67MiB를 확인 후 해당 디렉터리만 제거했다. 앱·프로필·실행 중 서버는 보존했다. 실제 네이티브 재검증과 CEF 22개 회귀는 사용자 키체인 승인 뒤 같은 바이너리로 이어야 하며, 이 객체 검사를 대신 성공으로 세지 않는다. 전체 목표는 active다.


## 2026-10-11, 재열기 승인 후 네이티브 검증과 단축키 전달 결함 수정

사용자의 ‘항상 허용’ 응답 뒤 v1 바이너리를 바꾸지 않고 탭 2의 HTTP load 완료를 확인했다. B3을 네이티브 닫기로, A2를 CLI로 닫은 뒤 메뉴는 새 사용자 탭 A4를, ⌘⇧T는 B5를 만들었다. task 소유권 없이 user 공간에 생성됐으며 빈 목록에서는 새 탭이 생기지 않고 메뉴가 비활성화됐다. A의 needle 찾기 1/8에서 B를 복구하면 찾기 막대가 닫히고 ⌘G로 되살아나지 않았다.

로컬 single-window fixture에서 실제 체크박스를 클릭한 후 ⌘W로 JavaScript Confirm을 표시했다. Cancel 후 탭 6은 그대로였고 복구 목록은 비었다. OK로 실제 닫은 뒤에는 같은 URL이 탭 7로 열렸다. borrowed 탭 4의 네이티브 닫기는 작업을 paused revision 2로 바꾸지만 복구 목록에는 넣지 않았다. AI 생성 탭 8의 scoped close와 finish --keep 이후 소유권 없는 완료 AI 공간 탭 9의 close도 제외됐다. 복구할 사용자 URL이 있는 상태에서 개요는 그대로였고 검색·북마크는 ‘검증’ 값과 입력 포커스를 보존했다.

active AI 탭 9에 ⌘⇧T를 보내면 새 탭은 없지만 active revision 3이 paused revision 4로 바뀌는 결함을 발견했다. 최초 ‘관찰기가 단축키보다 먼저 실행된다’는 해석은 소스 호출 순서와 달라 폐기했다. `src/mac.rs`는 handle_shortcut이 false인 경우에만 observe_page_input을 호출한다. 로컬 IME fixture에 알려진 Command+Shift+KeyT만 라틴/한글 Boolean으로 판별하는 기록을 추가했다. 사용자 본문에서 trusted KeyT, meta=true, shift=true, latin=true, hangul=false가 실제 전달됐다. AppKit 원본의 charactersIgnoringModifiers 값은 직접 기록하지 않았으므로 한글 입력기만의 문제라고 확정하지 않는다. 실제 IME 조합 검증과도 별개다.

`src/control_panel.rs`의 is_reopen_event는 정확한 Command+Shift 조합을 먼저 제한하고 `charactersByApplyingModifiers`로 현재 입력 소스에서 Command+Shift를 적용해 재번역한다. AppKit SDK 헤더에서 이 API가 keyCode·현재 layout을 사용하며 dead-key 상태를 바꾸지 않는다고 확인했다. 임의 물리 키코드나 한글 문자 대응표는 추가하지 않았다. 인식한 조합은 기존 unmodified text 분기보다 먼저 소비하며 비활성·repeat 처리도 유지한다. 독립 소스 검토에서 확정 P1/P2는 없었지만 실제 AI revision 보존과 본문 KeyT 차단은 수정 빌드로 검증해야 한다.

수정 소스 Rust 144개가 통과했고 전체 타깃 Cargo 검사도 통과했다. 근거는 `artifacts/reopen-v2-rust-tests.log`, `artifacts/reopen-v2-check.log`다. 기존 lib test의 dead_code 경고 두 개는 남아 있다. v1에서 결함을 찾았으므로 예약했던 CEF 22개 회귀는 v1에 실행하지 않았으며 v2에서 실행할 예정이다. v1 관찰·명령 원본과 실패는 `artifacts/reopen-v1-native.json`, `artifacts/reopen-v1-validation.json`에 보존했다.

검증용 탭 7·10·11과 AI 결과 9를 닫고 테스트 작업을 finished revision 5로 마쳤다. 종료 직전 사용자 탭 1만 남아 있었다. 이전 앱 세션 17169는 네이티브 종료 메뉴 뒤 exit 0이었으며 CUA 종료 후 AX 조회 timeout은 종료 실패로 해석하지 않았다. fixture 서버 22866도 정상 종료(exit 0)하고 같은 포트 49981에 진단 내용을 반영한 새 서버 세션 44316을 띄웠다. 원본 프로필과 사용자 탭 상태는 보존했다. 수정 release·정리 및 새 앱 실행 결과는 아래 후속 기록에서 갱신한다. 새 RSS, 외부 사이트, 최소 창, 실제 IME, Preseller 내장 검증은 이번에 실행하지 않았다. 커밋·푸시·배포는 하지 않았다.

수정 v2 release·서명·번들 교체가 성공했고 Cargo 중간 파일 4,332개·2.4GiB를 제거했다. 최종 SHA-256은 `84bfaf36d80d6b6048461b07fcbef5c5bca2862c3408aa7d2e772e1fa5204100`다. 현재 앱은 세션 87916, 메인 PID 30914 하나이며 앱 번들 1개·release 실행 파일 3개, target 약 2.7MiB·dist 약 335MiB·artifacts 약 11MiB다. 동일 프로필로 사용자 빈 탭 1과 이번 진단용 HTTP 탭 2만 있다. 로컬 첫 탐색은 5초 후 commit 전 WAIT_TIMEOUT이었고 새 SecurityAgent PID 30933이 확인돼 사용자의 직접 키체인 승인을 요청했다. 보안 창을 도구로 조작하지 않았다. 승인 뒤 같은 v2 바이너리로 실제 복구 단축키의 AI 상태·본문 전달을 확인하며, 이전 승인을 이유로 이 검증을 통과 처리하지 않는다.

v2 실행 상태·후속 절차는 `artifacts/reopen-v2-validation.json`, 승인 후 쓸 CLI 기록기는 `artifacts/reopen-v2-native-cli.py`, 고정 SHA를 확인하는 회귀 실행기는 `artifacts/reopen-v2-suite-run.py`다. 회귀 실행 전 이번에 만든 탭 2를 정리하고 원래 사용자 탭 1만 남겨야 한다. 회귀검사가 닫기 기록을 남길 수 있으므로 마지막 같은 바이너리 재실행 뒤 빈 복구 메뉴를 별도로 확인해야 한다. 최종 `cargo fmt --check`와 `git diff --check`도 통과했다. 이번 턴은 승인 이후의 검증·실제 결함 재현·소스 수정·새 빌드까지 진행한 상태이며 전체 목표는 active로 유지한다. 최종 CLI 상태는 사용자 공간·탭 2개이며 pendingWaits·capturesInFlight·pendingTabCreations는 모두 0이었다.

## 2026-10-11, 최근 닫은 사용자 탭 재열기 구현과 새 빌드 승인 대기

일상 브라우저 사용성의 다음 범위로 `파일 → 최근 닫은 탭 다시 열기`와 ⌘⇧T를 추가했다. 이전에는 Shift가 있어도 일반 ⌘T 분기로 들어갔다. 새 단축키는 검색·북마크 화면 해제와 기존 새 탭 분기보다 먼저 처리한다. 메뉴 validation과 실행 모두 사용자 공간, 닫힌 URL 존재, overlay 닫힘, 조합 중 아님을 확인한다. 비활성·반복 입력도 소비해 원치 않는 새 탭이나 AI handoff로 이어지지 않도록 했다.

`src/closed_pages.rs`는 HTTP(S) 주소 최대 10개와 token만 기억한다. 기존 주소 정규화를 재사용하며 주소당 8,192바이트를 넘기거나 자격증명이 있는 URL, about:blank 등 비웹 주소는 제외한다. 최근 URL payload 상한은 80KiB이며 진행 중 생성 사본·정규화 임시 할당·컨테이너 overhead는 별도다. Browser/Frame·본문·화면·renderer를 보관하지 않고 파일·DB·worker를 추가하지 않았다. 중복 재개를 막으며 생성 성공 시 정확한 token만 제거하고 실패하면 원본을 유지한다.

`src/browser.rs`는 실제 `on_before_close`에서 종료 당시 사용자 공간·소유권 없음·정상 탭 여부를 확인한다. CEF 156 로컬 바인딩 문서에서 main frame은 이 콜백 이후까지 유효함을 확인했고, STATE borrow 밖에서 실제 main frame URL을 읽는다. 닫기 요청만으로 기록하지 않으므로 beforeunload 취소는 항목을 만들지 않는다. CLI가 사용자 탭을 닫은 경우도 포함한다. AI·paused·borrowed·결과 공간 탭은 제외한다. 앱이 완전히 종료되면 메모리 기록은 사라지며, 앱 종료를 일부 탭이 취소하면 이미 닫힌 사용자 URL은 남는다. 다시 열기는 새 URL 탐색이며 폼·스크롤·뒤로가기·재실행 세션 복구는 아니다.

Rust lib 검사 144개가 통과했다. 새 7개는 순서·상한·부적합 URL·실패 보존·재진입·정확한 token 처리, 공간·작업 상태와 단축키 구분을 검사한다. 전체 타깃 Cargo 검사, format과 diff 공백 검사도 통과했다. 최초 단위 실행은 프로젝트 Ninja PATH 누락으로 CEF 빌드에 실패했고 `artifacts/reopen-v1-rust-tests-missing-ninja.log`에 보존했다. README의 기존 `.cache/bin` 경로를 적용한 재실행은 통과했다. 전체 타깃 검사에서 나온 새 unnecessary unsafe 경고를 제거한 최종 release는 경고 없이 성공했다. 기존 lib test의 사용되지 않는 함수 경고 두 개는 남아 있다. 독립 읽기 전용 검토에서 확정 P1/P2는 없었지만 실제 생성 실패와 native menu 검증의 일부는 아직 남았다.

이전 페이지 찾기 v2의 빈 사용자 탭 1만 있음을 확인한 뒤 네이티브 종료 메뉴를 눌렀고 앱 세션 97796의 exit 0을 확인했다. `artifacts/reopen-v1-build.log`의 release·서명 검증이 성공했으며 중간 파일 4,333개·2.4GiB를 정리했다. 새 바이너리 SHA-256은 `98dc57f68fd90783e86eafa832d05830e74311c77bb1fec4fa81e24f2ba25402`다. target 약 2.7MiB, dist 약 335MiB, artifacts 약 11MiB이며 원본 프로필은 보존했다.

새 앱 세션 17169, 메인 PID 23039에서 실제 파일 메뉴의 빈 목록 비활성화를 확인했다. 메뉴 상태에서 첫 Escape·단축키 시도는 AX 변화가 없어 성공으로 세지 않았다. 노출된 Cancel로 메뉴를 닫고 주소 필드를 직접 클릭한 뒤 ⌘⇧T를 눌러도 원래 빈 탭 하나와 주소 포커스가 유지됐다. 이는 빈 목록에서 기존 새 탭 분기로 새지 않는 네이티브 증거이며 실제 URL 복구의 성공은 아니다.

현재 직접 만든 탭 2의 `http://127.0.0.1:49981/find-in-page.html?reopen=A`는 5초 load 대기 뒤 `WAIT_TIMEOUT`, commit false·documentAvailable false·provisional true였다. SecurityAgent PID 23048이 실행돼 사용자에게 키체인의 ‘항상 허용’을 요청했다. 최신 바이너리와 로컬 fixture 세션 96572를 유지하며 추가 빌드는 하지 않는다. 승인 후 같은 바이너리로 URL 역순 재열기, AI 소유 탭 제외, overlay와 주소 초안 보존, beforeunload 취소, CEF 22개 회귀, 재시작 시 기록 소멸과 검증 데이터 정리를 이어야 한다. 실행하지 않은 항목을 통과로 기록하지 않는다.

기존 `tests/fixtures/single-window.html`을 동일 fixture 서버의 고정 경로로 연결했고 Python 구문 검사를 통과했다. 제품 설명과 메모리 한계, [재열기 설계](reopen-closed-tabs.md)를 갱신했다. 현재 원본 CLI·네이티브 기록은 `artifacts/reopen-v1-native.json`, 상태와 남은 단계는 `artifacts/reopen-v1-validation.json`이다. 실제 IME·좁은 창·외부 사이트·장시간 메모리·Preseller 내장은 미검증이다. 새 전체 RSS는 측정하지 않았으며 커밋·푸시·배포는 하지 않았다. 전체 목표는 계속 진행 중이다.

승인 질문 후 앱 세션 17169와 fixture 세션 96572를 직접 다시 조회해 실행 중임을 확인했다. 재실행 없이 같은 탭의 2초 load 대기도 commit 전 `WAIT_TIMEOUT`이었다. 해당 재확인 직전 상태의 대기 요청·캡처·탭 생성 예약은 0이었다. 앱 번들 하나와 release 실행 파일 3개만 남아 있고 최종 format·diff 공백 검사도 통과했다. 이번 턴은 구현·소스 검증·새 빌드의 제한된 네이티브 확인까지 진행했으며, 첫 HTTP 승인 대기를 전체 목표의 차단 완료나 제품 완료로 표시하지 않는다.

## 2026-10-11, 같은 바이너리의 입력 경계 검사와 IME 재현 도구 추가

페이지 찾기 v2의 승인된 바이너리를 유지하며 입력과 화면 검증을 이어갔다. SHA-256은 `458ee5ad7a075d2a0b3c35b1d75f037847976e7711720b7319529eea1e0ea427`로 같고, 제품 Rust 코드·서명·앱 실행 상태는 바꾸지 않았다. 목표 도구의 이번 조회는 `active`이며 전체 제품 완성을 뜻하지 않는다.

`tests/fixtures/ime-input.html`과 `tests/preview_fixture.py`의 고정 경로 `/ime-input.html`을 추가했다. 조합 시작·갱신·종료, beforeinput/input, keydown/up과 focus/blur의 상태·길이·신뢰 여부를 최근 20개까지만 표시한다. 입력 문자는 이벤트 기록에 복제하지 않으며 파일 저장이나 네트워크 전송은 없다. 정적 검사 및 격리 HTTP·종료 검사 7개를 통과했고 근거는 `artifacts/ime-input-fixture.json`이다. 이 7개는 실제 IME 검증과 구분한다.

새 검증 서버 `http://127.0.0.1:64990`의 문서를 이번에 만든 사용자 탭 3으로 열어 complete·loading false·provisional false를 확인했다. 네이티브 CUA의 `r`, `k` 입력은 `ㄱㅏ` 값과 trusted `beforeinput`/`input`의 `insertText` 각각 2회로 관찰됐다. `compositionstart/update/end`는 모두 0이므로 실제 IME 조합 검증을 통과로 기록하지 않는다. 소스 재검토에서 일반 키의 원본 NSEvent 전달과 별도 IME 재구현이 없음을 확인했지만, 자동화 경로와 제품 입력 처리 중 어느 쪽의 영향인지는 판정하지 못했다. native 주소·검색·북마크·찾기 입력의 조합 보호는 별도 검증이 필요하다.

같은 문서에서 ⌘F와 `한글검증표식` 붙여넣기는 `1/1`이었다. `가` 205자 붙여넣기는 AX상 200자, 결과 없음과 비활성 이전/다음으로 표시됐다. 사이드바를 접은 뒤에도 입력 포커스와 200자 값이 유지됐고 실제 화면의 결과·버튼이 겹치지 않았다. Esc로 막대를 닫고 사이드바를 복원했다. 네이티브 관찰과 CLI 원본은 `artifacts/input-layout-v1-native.json`에 있다.

좁은 창을 위한 테두리 드래그는 스크린샷의 2360×1664 픽셀 크기를 바꾸지 못했다. 창 바깥 경계 시도는 `windowNotFoundAtPosition`을 반환했다. AX에 노출된 표준 zoom 동작으로 확대와 원래 2360×1664 크기 복원은 확인했지만 최소 크기 화면의 증거는 아니다. 정적 배치 검토에서는 콘텐츠 최소 800×520pt에서 겹침을 발견하지 못했으나 실제 렌더링·클릭 검증과 구분한다. `docs/input-validation.md`에 직접 키보드와 좁은 창의 재현 절차·판정 기준을 남겼고 제품 계획과 찾기 문서에도 검증 경계를 반영했다.

이번 검증 탭 3만 닫아 기존 빈 사용자 탭 1, 사용자 공간, overview false와 pendingWaits·capturesInFlight·pendingTabCreations 모두 0으로 복원했다. 마지막 native AX도 원래 빈 탭 하나와 기본 화면을 표시했다. 직접 실행한 fixture PID 14398은 SIGTERM 뒤 세션 26854의 exit 0을 확인했다. 별도 fixture 격리 검사 서버도 종료됐다. 승인된 앱 메인 PID 11220은 그대로 유지하며 새 빌드나 앱 중복 실행은 없었다. target 약 2.7MiB, dist 약 335MiB, artifacts 약 11MiB로 확인했다. 종합 기록은 `artifacts/input-layout-v1-validation.json`이며 최종 공백 검사와 정리 상태 assertion이 통과했다. 원본 프로필을 보존했고 새 RSS·외부 사이트·전체 CEF 회귀는 이번 검사에서 다시 측정하지 않았다. Rust 137개와 CEF 22개는 앞선 v2 검증 결과다. 커밋·푸시·배포는 하지 않았다.

## 2026-10-11, 페이지 찾기 v2 승인 후 수정 검증과 정리 완료

사용자가 키체인에서 항상 허용한 뒤 기존 앱 세션 67307과 fixture 세션 69252가 살아 있음을 직접 확인하고 검증을 재개했다. SHA-256 `458ee5ad7a075d2a0b3c35b1d75f037847976e7711720b7319529eea1e0ea427`은 바뀌지 않았다. 기존 검증 탭 2의 실제 HTTP 문서가 complete, loading false, provisional false가 됐다. 승인 전 timeout과 goal blocked 기록은 아래에 당시 상태로 보존한다. 해당 승인 대기는 해소됐으며 전체 제품 목표가 완료됐다는 뜻은 아니다.

`artifacts/find-page-v2-native.json`에 같은 바이너리의 실제 네이티브 검사와 CLI 응답을 기록했다. 사용자 문서에서 `needle`은 1/8, Enter는 2/8, Shift Enter는 1/8이었다. AI A의 찾기는 A만 paused/revision 2로 바꿨다. 잘못된 A revision의 재개를 거절한 뒤에도 검색 필드와 다음 이동이 유지됐다. 다른 B의 handoff/revision 2와 재개/revision 3도 A의 검색어·포커스·이전 이동을 유지했다.

A를 올바른 revision 2로 재개하면 active/revision 3이 됐고 실제 AX에서 찾기 막대가 사라졌다. 같은 시점의 CUA 스크린샷에서 본문의 주황·노랑 강조가 없어지고 본문 높이가 복원된 것을 확인했다. ⌘G는 이전 검색어를 되살리지 않았으며 A와 B 모두 active/revision 3을 유지했다. 다시 ⌘F를 열면 빈 입력으로 시작하고 A만 새로 중지됐으며, 한글 붙여넣기는 1/5를 표시했다. 이는 실제 IME 조합 검사가 아니다. 검증 A/B는 finished/revision 5와 4, 소유 탭·closingTabs가 빈 상태로 정리했다.

같은 v2의 로컬 CEF 회귀는 작업 6개·공간 10개·단일 창 6개, 총 22개가 통과했다. 각 실행 exit 0, stderr 0바이트이며 `artifacts/find-page-v2-suite.json`에 있다. 최초 sandbox 시도는 읽기 전용 상태 조회가 소켓 권한 오류로 실패해 변경 전에 멈췄고, `*-sandbox.json`에 보존했다. 실행 권한을 받은 한 번의 재시도에서 통과한 결과와 구분한다. 검사 작업 7개는 모두 finished, tabs·closingTabs가 비었으며 원래 빈 사용자 탭 1과 대기·캡처·생성 예약 0을 확인했다. 근거는 `artifacts/find-page-v2-suite-state.json`이다.

네이티브 메뉴로 종료한 뒤 실행 세션 67307의 exit 0을 확인했다. 종료 직후 CUA 관찰은 timeout이었으므로 종료 증거로 사용하지 않았다. 같은 바이너리와 원본 프로필로 재실행한 세션은 97796, 메인 PID는 11220이다. 자체 HTTP 탭 2가 추가 승인 요청 없이 로드됐고 네이티브 화면에도 문서 제목이 표시됐다. 이 탭만 닫아 원래 빈 탭 1로 돌아왔으며 북마크 목록도 비어 있었다. 검증 서버 PID 96142를 SIGTERM으로 종료해 세션 69252의 exit 0을 확인했다. 최종 프로세스 조회에서 이전 메인 PID 7119와 fixture PID는 없고 새 메인만 남았다.

검사 후 빈 탭 상태의 프로세스는 6개, RSS 합계는 세 표본 모두 721.109MiB였다. 재시작·HTTP 열기/닫기·북마크 열기/닫기 뒤에도 6개였고 RSS는 694.438, 694.438, 694.625MiB였다. 각각 `artifacts/find-page-v2-after-tests-memory.json`, `artifacts/find-page-v2-restart-memory.json`에 조건을 기록했다. 공유 매핑을 중복 계산하는 RSS 합계이며 고유 RAM, 장시간 누수 검증이나 조건을 통제한 절감 측정이 아니다. 이전 v3 표본 696.672MiB와도 실행 시간과 사용 이력이 달라 성능 개선으로 비교하지 않는다.

승인 후 제품 소스와 바이너리를 수정하거나 재빌드하지 않았다. 기존 Rust 137개와 release 통과는 이전 소스 검사 결과로 유지한다. 최종 문서 공백 검사도 통과했다. README, 제품 계획, 페이지 찾기 설계와 메모리 문서를 갱신했으며 종합 결과는 `artifacts/find-page-v2-validation.json`이다. `dist`는 최신 앱 하나, `target/release`는 실행 파일 3개이며 target 약 2.7MiB, dist 약 335MiB, 검증 자료 약 11MiB다. 원본 프로필과 정상 북마크 백업 하나를 보존했다. 좁은 창, 실제 IME 조합, 외부 사이트, 장시간 부하와 Preseller 내장은 여전히 미검증이다. 커밋·푸시·배포는 하지 않았다.

## 2026-10-11, 페이지 찾기 v2 재개 취소 수정 빌드

AI 재개 때 이전 찾기를 취소하는 수정 후 `artifacts/find-page-v2-rust-tests.log`의 Rust 137개, format과 diff 공백 검사가 통과했다. 새 회귀는 실패한 재개 및 다른 작업 재개의 보존과 소유 작업 재개의 취소를 확인한다. native Browser 핸들이 없는 단위 검사이므로 실제 강조 해제와 막대 닫기의 증거는 아니다.

`artifacts/find-page-v2-build.log`의 release 빌드와 서명 검증이 성공했다. SHA-256은 `458ee5ad7a075d2a0b3c35b1d75f037847976e7711720b7319529eea1e0ea427`다. 이전 v1의 정상 종료를 확인한 뒤 교체했고 중간 파일 3,537개·1.9GiB를 정리했다. `target`은 약 2.7MiB, 번들은 약 335MiB, 검증 자료는 약 11MiB다. 원본 프로필을 보존했다.

새 앱 세션 67307, 메인 PID 7119에서 직접 만든 검증 탭 2로 같은 로컬 문서를 열었다. 최초 5초 load 대기는 `WAIT_TIMEOUT`, committed false·documentAvailable false·provisional true였고 새 SecurityAgent를 확인해 사용자에게 키체인 승인을 요청했다. `artifacts/find-page-v2-native.json`에 실패를 남겼고, native AX에서도 로딩 중인 검증 탭과 원래 빈 사용자 탭 1을 확인했다. 현재 새 바이너리의 네이티브 수정 검증과 CEF 회귀는 승인 후에 이어야 하며 통과로 표시하지 않는다.

승인 후 같은 바이너리를 검사하기 위해 최신 앱 한 개와 fixture 세션 69252(`http://127.0.0.1:61236`)를 유지한다. 이전 v1은 종료됐고 검증 AI 작업 두 개도 앞서 finished로 정리했다. README, 제품 계획과 페이지 찾기 설계를 갱신했다. 종합 상태는 `artifacts/find-page-v2-validation.json`에 있다. 실제 IME·좁은 창·외부 사이트·장시간 부하·새 RSS와 Preseller 내장은 미검증이며 전체 목표는 완료되지 않았다. 커밋·푸시·배포는 하지 않았다.

승인 응답을 기다린 뒤 같은 v2 탭의 3초 load 대기도 `WAIT_TIMEOUT`이었다. 새 탐색은 여전히 commit 전이며 대기 요청·캡처·탭 생성 예약은 모두 0이다. 앱이나 바이너리를 다시 실행·교체하지 않았다. 이 재확인은 승인 대기의 지속을 뜻하며 추가 성공 결과가 아니다.

다음 자동 재개에서도 앱 세션 67307과 fixture 세션 69252를 직접 조회해 둘 다 실행 중임을 확인했다. 같은 탭 2의 3초 load 대기는 여전히 commit 전 `WAIT_TIMEOUT`이며 자원 대기 수는 0이다. 이번 턴은 실행 핸들을 검증한 승인 대기로 분류한다. 바이너리·소스·앱 실행 상태를 바꾸지 않았고 같은 승인 질문을 다시 만들지 않았다.

세 번째 연속 goal 턴에서도 같은 두 실행 핸들이 살아 있고, 같은 SHA-256의 탭 2는 commit 전 `WAIT_TIMEOUT`이었다. 키체인 승인 의존이 해소되지 않아 goal 도구가 `blocked`를 반환했다. 완료로 표시한 것은 아니다. 재승인 후 현재 앱에서 HTTP 준비, AI 재개 시 막대·강조 해제와 다른 작업 보존, CEF 작업·공간·단일 창 회귀, 검증 데이터 정리를 이어야 한다. 앱·서버·프로필·바이너리는 그대로 유지하며 새 승인 질문이나 재실행은 하지 않았다. 남은 단계와 실행 핸들은 `artifacts/find-page-v2-validation.json`에 기록했다.

## 2026-10-11, 페이지 찾기 v1 실제 검증과 AI 재개 결함 수정

페이지 찾기 v1 release SHA-256은 `0c0659c2c476f9a88a57f21ca9911baf361004ddea0f97693e95b38fbb11625d`다. 빌드·서명 검증 뒤 중간 파일 4,332개, 2.4GiB를 정리했다. 첫 로컬 load 대기는 `WAIT_TIMEOUT`이었고 새 SecurityAgent가 확인됐다. 사용자가 항상 허용한 뒤 재빌드 없이 같은 탭 2에서 실제 요청 URL, complete와 loading false를 확인했다. 승인 전후 CLI 응답과 네이티브 관찰은 `artifacts/find-page-v1-native.json`에 남겼다.

실제 CUA 화면에서 `needle` 8개, `검색표식` 5개를 확인했다. 첫 입력에서 A 구역의 영문 일치가 주황색으로 선택됐으며 Enter, ⌘G, ⇧Enter, ⇧⌘G와 화살표 버튼으로 이동했다. 8→1과 1→8 순환도 확인했다. 관찰한 순서는 본문 6개 뒤 iframe 2개였고, 7번의 iframe 안 실제 강조를 스크린샷으로 확인했다. 검색어를 바꿀 때는 CEF의 현재 위치를 기준으로 일치를 선택하므로 항상 문서의 1번으로 이동하는 계약은 아니다. 한글 검색은 붙여넣기 검사다.

불일치 검색은 ‘결과 없음’과 비활성 화살표, 빈 입력은 개수 제거로 표시됐다. 짧은 검색어와 연속 다음·이전 후 다른 검색어로 바꿔도 최종 검색어의 결과가 표시됐다. Esc 후 같은 문서의 ⌘G는 검색어를 기억했다. 반면 탭 전환, 통합 검색, 북마크, 본문 링크 탐색과 찾기 대상 탭 종료 뒤에는 막대와 옛 검색어가 남지 않았다. 통합 검색과 북마크에서 ⌘F를 눌러도 해당 입력에 붙여넣기가 이어졌다. 네이티브 앱 활성화 전 최초 단축키가 AX 변화를 만들지 않은 시도와 오래된 AX ID로 실패한 클릭은 통과로 세지 않았다.

찾기를 연 상태에서 본문을 클릭해 실제 입력을 받았고 fixture의 trusted 이벤트를 확인했다. 현재 입력 소스에서 x 키가 한글 자모로 들어온 관찰은 남기되 정상적인 IME 조합·후보 선택 검증으로 해석하지 않았다. AI A에서 주소 초안을 만든 뒤 찾기를 열면 초안을 유지하고 A만 paused/revision 2로 바뀌며 B는 active/revision 1이었다. 이전 A revision은 `TASK_STALE_REVISION`으로 거절됐다. 찾기 후 본문에 붙여넣은 x도 전달됐고 추가 revision 증가는 없었다.

이미지 로드가 지연된 AI B에서 실제 load 대기 1개를 등록한 뒤 네이티브 ⌘F를 눌렀다. B만 paused/revision 2가 되고 기존 대기는 `TASK_CONTROL_LOST`, `outcomeUnknown:false`로 끝났으며 pendingWaits는 0으로 돌아왔다. A는 paused/revision 4를 유지했다. 이미지 gate는 자동 timeout 전에 직접 해제했다.

실제 검사에서 AI A의 재개가 기존 찾기 막대·결과와 대기 중 찾기를 만료시키지 않는 결함을 발견했다. `src/browser.rs`의 `TaskResume` 성공 결과에 속한 탭만 기존 `expire_tab_refs`로 정리하도록 수정했다. 실패한 revision과 다른 작업의 재개는 현재 찾기를 보존하고, 소유 작업의 성공한 재개는 현재·대기·늦은 결과를 취소하는 회귀 1개를 추가했다. 독립 정적 검토에서 추가 P1/P2는 확인되지 않았다. v1 검증을 이 수정의 실제 동작 증거로 합산하지 않는다.

검증 작업 두 개는 finished, 소유 탭과 closingTabs가 비었으며 원래 사용자 탭 1만 보존했다. 대기·캡처·생성 예약 모두 0을 확인한 뒤 네이티브 메뉴로 v1을 정상 종료했고 세션 63469의 exit 0을 확인했다. 수정한 v2의 빌드와 실행 검증은 이어서 기록한다. 사용자 프로필과 fixture 서버는 다음 검사에 사용하며, 커밋·푸시·배포는 하지 않았다.

## 2026-10-11, 페이지 내 찾기 구현과 소스 검사

일상 브라우저 사용성의 다음 범위로 페이지 내 찾기를 구현했다. `src/page_find.rs`는 검색어 최대 200자, 진행 중 요청과 최신 후속 요청 하나, 문서 세대와 token 검사를 맡는다. `src/browser.rs`는 CEF FindHandler와 기존 사용자 개입 흐름을 연결하고 탐색·닫기·제어권 변경 때 찾기를 취소한다. `src/control_panel.rs`에는 주소줄 아래 44px 네이티브 찾기 막대, ⌘F, 다음·이전, 결과 수, Esc와 같은 문서의 마지막 검색어 기억을 추가했다. 새 파일 저장, 상주 worker나 UI용 웹 renderer는 추가하지 않았다.

고정된 CEF·Chromium 소스를 확인해 최초 검색이 총수만 세지 않고 첫 결과를 선택하도록 연결했다. 네이티브 입력 포커스, 실제 first responder의 IME 조합 검사, AI handoff 후 주소 초안 세대 보존, 오류 때 정확한 이전 요청 정리도 반영했다. 이는 소스 구현 설명이며 실제 CEF 검색 결과와 IME 성공을 뜻하지 않는다. `docs/page-find-design.md`와 `artifacts/find-page-test-plan.md`에 계약과 후속 검사를 기록했다.

`artifacts/find-page-v1-rust-tests.log`의 Rust 136개가 통과했다. 기존 128개에 찾기 상태 6개와 UI 상태 2개를 더했다. 전체 타깃 Cargo 검사, format과 diff 공백 검사도 통과했다. lib test의 사용되지 않는 함수 경고 2개는 남아 있다. 별도 fixture HTTP 검사 5개는 통과했으나 HTML 전달·경로·종료 확인이며 브라우저 검색 증거가 아니다. `artifacts/find-page-v1-fixture.json`에 결과를 남겼다.

교체 전 v3의 빈 사용자 탭 1만 남아 있음을 확인했다. 이 상태에서 RSS 합계는 6개 프로세스, 696.672MiB였고 `artifacts/find-page-v1-before-memory.json`에 조건을 기록했다. 공유 매핑이 중복된 합계이며 새 기능의 절감 수치나 통제된 전후 비교가 아니다. 이전 앱은 네이티브 앱 메뉴로 종료했고 실행 세션 19719의 exit 0을 확인했다. 새 release 빌드와 실제 네이티브·CEF 검증 결과는 아래에 이어 기록한다. 사용자 프로필은 보존했으며 커밋·푸시·배포는 하지 않았다.

## 2026-10-11, 사용자 승인 후 최종 v3 HTTP 포커스 검증

사용자가 키체인에서 항상 허용한 뒤 실행 중인 v3를 재빌드하거나 교체하지 않고 검증을 재개했다. SHA-256은 `93bbf74352f3565713a24b4c9b20729e2b7bb627f8b771c7f91f35ebb9b9a9bd`로 유지됐다. 기존 검증 탭 3이 요청한 로컬 HTTP URL로 이동하고 readyState complete, loading false, provisional false가 됐다. 아래 승인 전 timeout과 blocked 기록은 당시 상태로 보존한다. 키체인 차단은 이번 승인으로 해소됐으며 향후 다른 바이너리의 승인이나 코드 서명 문제가 해결된 것은 아니다.

`artifacts/search-preview-v3-native.json`에 같은 앱의 네이티브 CUA 검사와 CLI 응답, 북마크 파일 스냅샷을 추가했다. 통합 검색에 사용자 탭 3, AI 탭 5와 북마크 4가 각각 표시됐다. 원본 사용자 탭 3을 CLI로 닫은 뒤에도 검색 필드 포커스가 유지됐고 추가 붙여넣기가 반영됐다. ‘저장’으로 북마크 하나를 찾고 Enter를 누르면 새 사용자 탭 6이 열렸다. AI 작업은 active/revision 1, 선택 탭 5를 유지했다.

북마크 이름 편집 중 탭 6을 닫아도 선택된 이름 입력과 원래 URL A가 보존됐다. ‘초안 유지’로 이름을 바꿔 저장하고 JSON에서 같은 id 4와 URL을 확인했다. 이전 빈 탭 회귀에 더해 로드된 HTTP 문서와 실제 이름 편집을 확인한 결과다. 한글은 붙여넣기로 입력했으며 IME 조합의 증거는 아니다.

직접 만든 북마크는 UI로 삭제해 본문 `items: []`, `nextId: 5`를 확인했다. 정상 백업 하나는 저장 정책대로 유지했다. 검증 작업 `task-56309256e95d607ba9d5e0e0a50a309f`는 finished/revision 2, 탭 0이며 원래 사용자 탭 1을 보존했다. 최종 v3 CEF 회귀는 작업 6개·공간 10개·단일 창 6개, 총 22개 모두 통과했다. 각 실행 exit 0, stderr 0바이트이며 `artifacts/search-preview-v3-suite.json`에 있다. 회귀가 생성한 작업 7개는 모두 finished, tabs와 closingTabs가 비어 있고 기존 사용자 탭 1만 남았다. 대기 요청, 캡처, 탭 생성 예약은 모두 0이다. 최종 상태 근거는 `artifacts/search-preview-v3-suite-state.json`이다.

v3 앱을 CUA의 ⌘Q로 종료하고 세션 20721의 exit 0을 확인했다. 직접 실행한 fixture PID 76092도 SIGTERM 후 세션 47479에서 exit 0을 확인했다. 원본 프로필로 같은 바이너리를 재실행한 세션은 19719, 메인 PID는 88794다. 네이티브 화면에 빈 사용자 탭 하나와 빈 북마크 목록이 표시됐고, 백업에 남은 삭제 항목은 되살아나지 않았다. 목록을 닫고 기본 브라우저 화면으로 복귀했다. 최종 프로세스 확인에서 이전 앱과 fixture 서버는 없었으며 앱 메인 하나만 남았다. `dist`에는 최신 번들 하나, `target/release`에는 실행 파일 3개뿐이다. `target` 약 2.6MiB, `dist` 약 335MiB, 검증 자료 약 10MiB이며 사용자 프로필은 보존했다. 이번 재개에는 새 빌드나 빌드 캐시 증가가 없다. 재실행 후 HTTP 탐색은 검사하지 않았다.

이번 재개에서 제품 소스는 바꾸지 않았다. 종합 근거는 `artifacts/search-preview-v3-validation.json`이며 README, 북마크 설계, 제품 계획과 메모리 문서의 승인 대기 상태를 갱신했다. 기존 Rust 128개와 빌드 결과는 소스 변경이 없어 재실행하지 않았다. 새 RSS 측정, 외부 사이트, 장시간 부하, IME와 Preseller 내장은 이번 범위에서 검증하지 않았다. 전체 제품 목표는 완료되지 않았으며 커밋, 푸시, 배포는 하지 않았다.

## 2026-10-11, 포커스 수정 v3 빌드와 빈 탭 네이티브 회귀

`src/control_panel.rs`의 responder 정리 수정을 v3에 반영했다. Rust 128개와 전체 target 컴파일, format과 공백 검사가 통과했다. 초기 sandbox의 Unix 소켓 검사 3개 실패와 권한을 받아 재실행한 통과는 아래 기록 및 각각의 로그에 남겼다. lib test의 `bookmark_completed` dead_code 경고 1개는 유지된다.

`artifacts/search-preview-v3-build.log`의 release 빌드와 서명 검증이 성공했다. 바이너리 SHA-256은 `93bbf74352f3565713a24b4c9b20729e2b7bb627f8b771c7f91f35ebb9b9a9bd`다. 이전 v2는 세션 53428에서 exit 0으로 종료했고 새 빌드가 중간 파일 4,332개, 2.4GiB를 제거했다. `target` 약 2.6MiB, `dist` 약 335MiB, `artifacts` 약 10MiB이며 사용자 프로필은 보존했다.

v3에서 새 빈 사용자 탭 2를 선택하고 통합 검색을 연 뒤 CLI로 탭 2를 닫았다. 검색 필드의 포커스가 유지됐고 붙여넣은 ‘포커스유지’가 입력과 결과에 반영됐다. 별도 빈 탭 4를 선택한 뒤 북마크 목록에서 ‘목록유지’를 입력하고 탭 4를 닫았을 때도 포커스를 유지했으며 ‘계속’을 추가로 붙여넣어 ‘목록유지계속’이 됐다. 이는 about:blank의 실제 네이티브 회귀 증거다. 로드된 HTTP 문서와 북마크 이름 초안 편집의 검사로 확대하지 않는다. 근거는 `artifacts/search-preview-v3-native.json`이다.

탭 4의 첫 닫기 명령은 화면 순번 3과 CLI ID 4가 다르다는 자동 승인 검토로 실행 전 거절됐다. 별도의 읽기 전용 탭 조회에서 ID 4가 이번에 생성한 빈 검증 탭임을 확인한 뒤 같은 대상의 닫기가 승인돼 성공했다. 원래 탭 1과 로컬 탐색 검증 탭 3은 보존했으며 해당 닫기 명령의 자동 승인 차단은 해소됐다.

검증용 HTTP 탭 3은 5초 DOM 대기 후 WAIT_TIMEOUT, 실제 URL about:blank와 provisional true를 유지했다. 새 SecurityAgent를 확인해 사용자가 키체인에서 직접 항상 허용하도록 요청했다. 빈 문서에서 남은 준비 플래그를 새 HTTP 문서 완료로 해석하지 않는다. 최종 상태는 사용자 탭 1 선택, 원래 탭 1과 검증 탭 3 두 개, pending waits, 탭 생성 예약, 캡처 모두 0이다. 새 앱 세션 20721, 메인 PID 85207과 로컬 fixture 세션 47479는 승인 후 같은 바이너리 검증을 위해 유지했다. 이전 앱과 검증을 마친 작업들은 종료했다. v3의 로드된 문서 검색, 북마크 편집 포커스와 CEF 회귀는 아직 남아 있다.

후속 대기 확인에서 앱 세션 20721과 fixture 세션 47479가 모두 실행 중임을 같은 핸들로 확인했다. 재실행하지 않았으며 탭 3의 3초 DOM 대기는 다시 `WAIT_TIMEOUT`, 실제 URL about:blank와 provisional true였다. 수정 A HTTP GET은 아직 도착하지 않았다. 기존 키체인 승인 질문은 그대로 대기하며 의존하는 HTTP와 편집 검증을 통과로 기록하지 않는다.

최초 v3 실행과 두 번의 후속 목표 턴에서 같은 승인 대기를 연속 세 번 확인했다. 마지막 확인에서도 앱 세션 20721이 실행 중이고 SecurityAgent PID 85219가 남아 있었으며, 탭 3의 1초 DOM 대기는 WAIT_TIMEOUT이었다. HTTP 문서, 북마크 이름 편집과 최종 CEF 회귀를 계속하려면 사용자 승인이 필요하다. 독립적으로 가능한 소스 검사와 빈 탭 포커스 검증은 마쳤다. 이 상태에서 새 바이너리나 검증되지 않은 기능을 추가하지 않으며, 기존 승인 질문과 현재 앱 및 로컬 fixture를 유지해 승인 후 같은 바이너리로 재개한다.

`artifacts/search-preview-v3-validation.json`과 제품 문서에 현재 범위 및 이전 v2 증거를 구분했다. 확대 UI 초안은 제거한 상태이며 이번 앱에는 포함되지 않는다. 커밋, 푸시, 배포는 하지 않았고 전체 목표는 진행 중이다.

목표 도구가 사용자 승인 대기 상태인 `blocked`를 반환했다. 전체 목표를 완료로 표시하지 않았다.

## 2026-10-11, 통합 검색과 지연 미리보기 실행, 탭 닫기 포커스 수정

사용자가 키체인을 승인한 뒤 검색과 미리보기 v2 SHA-256 `f9168ced388db274d3d18538a792081c7b5302bffc6b7817b4a59616c9227f74`를 재빌드하지 않고 검증했다. 로컬 문서는 complete가 됐고 HTTP GET 200을 확인했다. 승인 전 5초, 3초 DOM 대기의 `WAIT_TIMEOUT`은 `artifacts/search-preview-v2-native.json`에 그대로 보존한다.

혼합 검색에서 사용자 탭 2, AI 탭 3과 북마크 id 3이 종류 표시와 함께 구분됐다. 방향키와 Enter로 AI 탭을 선택한 뒤 북마크를 선택하면 기존 사용자 탭으로 돌아왔다. AI 작업은 active, revision 1, 선택 탭 3을 유지했다. 사용자 탭을 닫은 뒤 북마크를 열면 새 사용자 탭 4가 생겼으며 같은 숫자의 탭과 북마크가 충돌하지 않았다. Tab, Shift+Tab 후 검색 입력, 결과가 없을 때 Enter, 북마크 행 마우스 열기도 확인했다. 한글은 붙여넣기이며 IME 조합 검증이 아니다.

다만 검색 중 현재 사용자 탭을 CLI로 닫으면 결과와 검색어는 갱신됐지만 입력 포커스가 창으로 이동하고 추가 붙여넣기가 반영되지 않았다. 이 실패를 성공으로 합산하지 않았다. `src/control_panel.rs`의 `detach_view → clear_view_responders`가 기억된 active 탭이라는 이유만으로 네이티브 검색 입력까지 해제하던 경로를 수정했다. 이제 제거할 CEF 뷰 내부의 실제 responder만 해제한다. initial responder의 자손 검사와 borrow 밖의 뷰 정리 순서는 유지하며 강제 재포커스나 새 타이머를 만들지 않았다. 독립 정적 검토에서 새 P1/P2 문제는 확인되지 않았지만 수정 후 바이너리 검증은 아직 남아 있다.

`tests/preview_fixture.py`의 빠른 문서, 헤더 지연, 이미지 지연을 서로 다른 작업 공간에서 실행했다. 헤더 대기는 미확정 문서, 이미지 대기는 DOM interactive이지만 메인 로드 미완료였다. 서버 상태에서 두 요청이 실제로 대기 중일 때 빠른 문서만 파란색 실제 미리보기를 표시하고 나머지 카드는 placeholder를 유지했다. 헤더 POST 해제 후 주황색 미리보기, 이미지 POST 해제 후 SVG를 포함한 초록색 미리보기가 자동으로 생성됐다. 공간 화면을 다시 열지 않았다. 두 gate의 자동 timeout은 0이고 최종 load 대기는 모두 성공했다. 네이티브 AX와 스크린샷, CLI 상태와 HTTP gate 기록은 `artifacts/search-preview-v2-delays.json`에 구분해 남겼다. 과거 키체인 승인 중 캡처 정체의 원인이나 일반적인 자동 복구를 확정한 검증은 아니다.

추가 로컬 CEF 회귀는 `artifacts/search-preview-v2-suite.json`에서 tasks 6개, spaces 10개, single_window 6개, 총 22개가 통과했고 세 실행 모두 exit 0, stderr 0바이트였다. `artifacts/search-preview-v2-suite-state.json`에 검사 전후 탭과 자원 상태를 남겼다. 이 v2 결과는 소스에만 있는 포커스 수정의 통과 결과가 아니다.

검증 북마크 id 3은 UI로 삭제했고 본문은 빈 목록, nextId 4다. 통합 검색 작업과 지연 검사 작업 세 개는 finished, 소유 탭 0으로 정리했다. 원래 사용자 탭 1만 남겼고 pending waits, 탭 생성 예약, 진행 중 캡처는 0이다.

포커스 수정 소스의 Rust 검사는 처음 sandbox에서 125개 통과, 임시 Unix 소켓 생성 3개가 `Operation not permitted`로 실패했다. 실패 출력은 `artifacts/search-preview-v3-rust-tests-sandbox.log`에 보존했다. 같은 검사를 소켓 생성 권한이 있는 환경에서 다시 실행해 `artifacts/search-preview-v3-rust-tests.log`의 128개가 통과했다. 소스 format과 공백 검사도 통과했다. 이 결과는 AppKit 포커스 동작의 실제 재검증을 대신하지 않는다. v2 앱은 ⌘Q로 종료했고 세션 53428에서 exit 0을 확인했다.

승인 대기 중 페이지 확대 API도 조사했다. 기본 CEF 확대의 공유 host 동작이 다른 AI 탭에 영향을 줄 수 있어 잠시 작성한 네이티브 UI 초안은 제거했다. 확대 기능은 구현했다고 보고하지 않는다. 설치 공개 헤더와 현재 공식 소스의 근거, 고정된 CEF 구현과 실제 동작의 미검증 범위는 `docs/product-plan.md`에 기록했다. 현재 앱을 이 조사 때문에 다시 빌드하지 않았다.

## 2026-10-11, 통합 검색과 미리보기 보호 v2 빌드, 승인 대기

통합 검색과 미리보기 시작 조건을 release 앱에 반영했다. 기존 북마크 v1 앱은 정상 종료했고 실행 세션 59025에서 exit 0을 확인했다. `artifacts/search-preview-v2-build.log`의 release 빌드와 번들 서명 검증이 성공했으며 최신 바이너리 SHA-256은 `f9168ced388db274d3d18538a792081c7b5302bffc6b7817b4a59616c9227f74`다. 빌드 스크립트는 이전 번들을 교체하고 중간 파일 1,263개, 669.4MiB를 제거했다. 디스크 사용량은 `target` 2.6MiB, `dist` 335MiB, `artifacts` 약 10MiB다.

새 앱의 네이티브 사이드바에 `탭·북마크 검색 (⌘⇧A)`이 표시되는 것을 확인했다. 원본 프로필에서 검증용 사용자 탭 2를 만들어 로컬 fixture를 열었으나 5초 DOM 대기는 `WAIT_TIMEOUT`이었다. 실제 URL은 `about:blank`, 요청 URL은 fixture, provisional은 true였으며 HTTP GET은 도착하지 않았다. 새 SecurityAgent 프로세스도 확인해 사용자에게 이 바이너리의 키체인 승인을 요청했다. 남은 문서 준비 플래그를 새 문서 완료로 해석하지 않는다. 근거는 `artifacts/search-preview-v2-native.json`이다. 새 바이너리에서 실제 혼합 검색, 지연 미리보기와 CEF 회귀 검사는 아직 완료하지 않았다.

`tests/preview_fixture.py`를 추가했다. 127.0.0.1의 빠른 페이지, 헤더 지연 페이지, 이미지 지연 페이지와 상태 조회, 개별 POST 해제를 제공한다. 요청은 최대 120초를 기다리고 종료 신호에서 대기와 서버 스레드를 정리한다. 임의 경로의 파일은 제공하지 않는다. 문법 검사와 별도 임시 서버의 HTTP, 대기 해제, 종료 검사 4개가 통과했다. 근거는 `artifacts/preview-fixture-smoke.json`이다. 최초 sandbox bind 거절은 priorAttempt로 보존했고 권한을 받아 재실행했다. 검증용 서버 두 개는 exit 0으로 종료하고 임시 로그를 제거했다. CEF 문서 준비, 네이티브 미리보기와 120초 자동 timeout의 성공 증거는 아니다.

승인 대기 후속 확인에서 앱 세션 53428과 fixture 세션 47479가 모두 살아 있음을 같은 실행 핸들로 확인했다. 재실행하지 않았으며 같은 탭의 3초 DOM 대기는 다시 `WAIT_TIMEOUT`이었다. HTTP GET은 아직 도착하지 않았고 실제 URL은 `about:blank`, provisional true를 유지했다. 기존 승인 질문은 대기 상태로 두며 새 빌드나 추가 기능 변경은 시작하지 않았다. 이는 대기 상태 재확인이며 새 기능 통과 결과가 아니다.

소스의 Rust 128개 검사와 전체 target 컴파일 통과는 아래 소스 검증 기록과 같다. 북마크 v1의 실제 UI, 재시작과 CEF 22개 통과를 v2 결과로 확대하지 않는다. 사용자 프로필과 빈 사용자 탭 1을 보존했다. 최종 CLI 상태에서 검증용 탭 2와 원래 탭 1, 총 2개이며 pending waits, 생성 예약, 캡처는 모두 0이다. 승인 후 검증을 위해 최신 앱 세션 53428과 127.0.0.1:57075 fixture 서버 세션 47479를 유지한다. 이전 앱과 임시 검증 서버는 종료했다. `README.md`, 제품 계획, 메모리와 북마크 설계 문서에도 v2 빌드와 미완료 범위를 반영했다. 커밋, 푸시, 배포는 하지 않았으며 전체 목표는 진행 중이다.

## 2026-10-11, 공유 북마크 화면과 비동기 저장 연결

승인된 JSON 저장을 기존 AppKit 브라우저 창에 연결했다. `src/bookmark_runtime.rs`는 Store 하나를 소유하고 파일 읽기와 변경마다 짧은 worker 하나를 사용한다. 결과 채널은 한 칸이며 정상 종료 전에 진행 중인 파일 작업을 기다린다. 새 상주 프로세스나 UI용 renderer는 추가하지 않았다.

`src/control_panel.rs`에 사이드바 버튼, 북마크 메뉴, ⌘D, 고정 URL 이름 편집, 제목과 URL 검색, 추가, 삭제, 상태 안내를 연결했다. 검색은 200자, 결과는 30개로 제한한다. 오래된 행 버튼과 완료 token이 다른 항목이나 새 편집 초안에 적용되지 않게 검사한다. 준비 실패 때도 숨은 CEF 페이지에서 포커스를 거둔다. `src/browser.rs`는 같은 URL의 정상 사용자 탭을 재사용하고 없으면 내 공간에 새 탭을 연다. AI 소유 탭, 중지와 불완전 탐색 탭은 재사용하지 않는다. `src/main.rs`, `src/lib.rs`에는 시작과 종료를 연결했다.

독립 소스 검토에서 발견한 오래된 행, 삭제한 초안, 늦은 미리보기, 중지된 탭 재사용 문제를 보완했다. `artifacts/bookmarks-v1-rust-tests.log`의 124개 검사가 통과했다. v6의 117개에 runtime 3개, 탭 재사용 2개, UI 완료 상태 2개가 추가됐다. release 빌드와 서명 검증이 성공했고 `artifacts/bookmarks-v1-build.log`에 중간 파일 3,537개, 1.8GiB 정리가 기록됐다. 앱 SHA-256은 `bd7baf100fd434e0898fa98c9453988fb5b531ed205d5eea86f4e27f6c828dbd`다.

### 같은 바이너리에서 확인한 결과

빈 페이지의 ⌘D는 오류를 표시하고 북마크 검색 필드에 포커스를 유지했으며 Esc로 돌아왔다. 최초 로컬 페이지 탐색은 HTTP 요청 전 대기 상태였고 SecurityAgent가 실행됐다. 사용자가 키체인을 승인한 뒤 위 SHA-256의 같은 바이너리에서 페이지가 열렸다. 승인 전 두 번의 `WAIT_TIMEOUT`과 승인 후 결과를 모두 `artifacts/bookmarks-v1-native.json`에 보존했다. 당시 상태의 `committed`와 문서 준비 플래그는 기존 빈 문서에서 남은 값이며, 실제 URL은 `about:blank`, 요청 URL은 fixture, `provisional: true`였다. 이를 새 요청이 commit한 상태로 해석하지 않는다.

- 로컬 CEF 회귀: `artifacts/bookmarks-v1-suite.json`의 tasks 6개, spaces 10개, single_window 6개, 총 **3묶음 22개 검사**가 통과했다. 각 실행의 exit code는 0이며 기존 사용자 탭을 보존했다. 배경 탭 스크린샷이 그 탭의 전경 기준과 같고 다른 탭의 전경과 다름을 확인한 결과도 포함한다.
- 네이티브 추가와 편집: ⌘D가 로컬 페이지 제목과 확정된 URL을 제안했다. 빈 이름은 오류와 편집 초안을 유지했고, 한글 이름으로 수정해 저장했다. 같은 URL을 다시 추가하면 id 1을 편집하며 중복 항목을 만들지 않았다. 이름 변경 후 이전 정상 내용이 백업에 남았다.
- 네이티브 검색과 열기: 제목, URL에 걸친 여러 검색어, 일치하지 않는 결과에서 Enter, 방향키 선택, Enter와 마우스 열기를 확인했다. 기존 사용자 탭을 재사용했고, 사용자 탭만 닫은 상태에서는 새 사용자 탭을 만들었다. AI 공간에서도 같은 북마크를 읽었으며 AI 소유 탭을 재사용하거나 작업을 중지하지 않고 active/revision 1을 유지했다.
- 초안 보존: 북마크 A 편집 중 CLI로 원본 탭을 B로 탐색해도 초안의 URL A를 유지했고 저장은 A의 이름만 바꿨다. 별도 B는 id 2로 추가됐다.
- 실제 앱 재시작: 정상 종료 후 같은 바이너리를 다시 실행해 두 이름과 id, 순서가 보존되는 것을 확인했다. 마우스 열기는 추가 승인 요청 없이 로컬 페이지를 불러왔다. 검증 항목 두 개를 UI로 삭제한 뒤 다시 정상 종료, 실행해 빈 목록을 확인했다. 백업에 마지막 항목이 남아 있어도 정상 빈 본문을 우선해 삭제한 항목이 되살아나지 않았다.

위 네이티브 확인은 `artifacts/bookmarks-v1-native.json`의 6개 수기 확인 묶음, CLI 응답과 파일 스냅샷에 근거한다. 이 결과는 로컬 fixture와 해당 네이티브 동작에 한정한다. 한글 IME 조합, 외부 사이트, 로그인, 장시간 부하, 전원 차단 내구성과 Preseller 내장은 검증하지 않았다. 모의 DOM, Python 격리 검사를 이번 북마크 작업에서 새로 실행한 것으로 보고하지 않는다.

### 손상 저장소의 읽기 전용 동작

별도 임시 프로필에서 손상된 본문과 정상 백업을 준비해 앱을 실행했다. 손상 안내와 읽기 전용 상태가 표시되고 추가, 이름 변경, 삭제 버튼이 비활성화됐으며 ⌘D도 편집기를 열지 않았다. 복구 목록의 북마크는 Enter로 사용자 탭에서 열 수 있었다. 검사 전후와 정상 종료 후 본문, 백업 SHA-256이 같아 원본 바이트 보존을 확인했다. 종료 exit 0 뒤 해당 임시 프로필을 삭제했다. 근거는 `artifacts/bookmarks-v1-recovery.json`이며 사용자 원본 프로필의 손상, 복구를 시험한 결과가 아니다.

### 메모리와 정리

검사에 사용한 북마크 두 개는 UI로 삭제했다. `tasks-phase1` 프로필의 본문은 `items: []`, `nextId: 3`이며 마지막 정상 항목을 담은 백업 하나는 저장 정책에 따라 보존했다. 검증 작업 `task-829cbd2b91c608a7a2b26fb70f6da06d`는 finished/revision 2, 소유 탭 0으로 끝났고 모든 검증 탭을 닫았다. 정리 시 사용자 빈 탭 하나를 유지했고 pending waits, 생성 예약, 캡처는 모두 0이었다.

같은 바이너리를 빈 목록과 빈 탭 하나로 재실행한 뒤 `artifacts/bookmarks-v1-fresh-memory.json`의 3회 표본은 각각 6개 프로세스, RSS 합계 689.406, 689.406, 689.469MiB를 기록했다. 최고값은 **689.469MiB**다. 공유 매핑이 중복 계산될 수 있는 RSS 합계이며 고유 RAM이 아니다. 이전 v6의 검사 직후 731.312MiB와는 실행 조건이 달라 메모리 절감이나 누수 없음의 증거로 사용하지 않는다.

첫 실행 세션 32300, 보존 확인 재실행 9385, 빈 목록 재실행 28086, 격리 복구 실행 48248은 각각 ⌘Q 이후 exit 0을 확인했다. 로컬 HTTP 서버 세션 3644도 종료해 exit 0을 확인했다. 원본 프로필로 최종 재실행한 세션 59025의 메인 PID는 72864다. `artifacts/bookmarks-v1-native.json`의 마지막 CLI 상태는 내 공간의 탭 1만 선택, 탭 총 1개, overview 꺼짐, pending waits, 생성 예약, 캡처 0이다. 해당 앱의 프로세스 트리 6개만 남았고 이전 Naru 프로세스는 없었다. `dist`의 바이너리는 위 북마크 v1 SHA-256 그대로다. 최종 디스크 사용량은 `target` 약 2.6MiB, `dist` 약 335MiB, `artifacts` 약 10MiB다. 이전 v6의 정상 종료와 더 이전 v4의 watchdog, exit 2 기록은 아래에 별도로 보존했다.

`README.md`, 북마크 설계, 제품 계획을 갱신했으며 사용자 프로필은 보존했다. 커밋, 푸시, 배포는 하지 않았다. 전체 목표는 계속 진행 중이다.

## 2026-10-11, 후속 통합 검색과 미리보기 시작 조건, 소스 검증

`src/tab_search.rs`, `src/control_panel.rs`에서 열린 탭 검색에 공유 북마크 결과를 연결했다. `Tab`과 `Bookmark` ID를 구분하고 열린 탭을 먼저 표시하며 합산 결과를 40개로 제한한다. 북마크는 기존 runtime의 캐시에서 최대 30개만 읽는다. 로딩, 저장, 읽기 전용 안내를 표시하면서 탭 검색을 유지하고, 선택한 결과의 삭제, 변경을 반영한다. 낡은 행 callback은 버튼 identity로 거절하며 브라우저와 북마크 runtime 호출 전에 SHELL borrow를 해제한다. 검색 중 CEF 포커스, 본문 handoff 차단과 IME 조합 가드를 유지했다.

`src/browser.rs`의 UI 미리보기는 DevTools observer가 등록되고 현재 문서가 commit, 문서 준비, 메인 로드를 마친 뒤에만 시작하도록 보완했다. provisional, 미완료 탐색, 중지, 오류, renderer 종료 상태는 제외한다. CEF 로딩 여부는 STATE borrow 밖에서 읽고, 예약 직전에 epoch, 공간, generation, observer, 문서 준비와 단일 캡처 자리를 다시 검사한다. 미준비 후보는 시도 기록과 캡처 자리를 소비하지 않아 다음 준비된 후보를 처리할 수 있다.

이 변경은 승인 대기 중 시작한 과거 미리보기 요청이 남았던 기록에 대한 **시작 조건의 예방책**이다. 당시 stuck의 원인을 확정하거나 자동 복구를 입증하지 않는다. 진행 중인 캡처는 기존처럼 실제 결과, detach, close까지 자리를 유지하며 timeout이나 renderer 종료만으로 반환하지 않는다. 초기 observer 미등록 및 commit 직후의 미준비 문서가 기존 조건을 통과하던 경로를 차단한다.

두 소스의 독립 정적 검토에서 확인된 P1/P2 문제는 없었다. `artifacts/search-preview-v2-rust-tests.log`에서 Rust 검사 **128개**가 통과했다. 북마크 v1의 124개에 통합 검색 2개와 미리보기 준비, 예약 2개가 추가됐다. `artifacts/search-preview-v2-check.log`의 `cargo check --all-targets`도 성공했다. lib test에서 `bookmark_completed` 미사용 `dead_code` 경고 1개가 남았으며 경고 없는 검증으로 보고하지 않는다.

이 소스는 아직 release 앱으로 빌드하거나 실행하지 않았다. 위 북마크 v1의 22개 로컬 CEF 검사, 네이티브 결과, 메모리 수치를 후속 소스의 통과 결과로 확대하지 않는다. 통합 결과의 실제 클릭, 포커스, IME 조합, 지연 로딩 페이지에서 미리보기 시작 차단과 완료 후 생성은 새 바이너리에서 확인할 남은 작업이다. `artifacts/search-preview-v2-cleanup.log`에 Cargo 중간 산출물 3,073개, 약 1.8GiB 제거와 최신 release 실행 파일 보존을 기록했다. 사용자 프로필과 기존 북마크 v1 앱 번들은 보존했으며 커밋, 푸시, 배포는 하지 않았다.

## 2026-10-11, 열린 탭 검색과 JSON 북마크 저장 모듈

사용자는 모든 공간이 공유하는 북마크를 기존 앱 데이터 폴더의 JSON 파일과 정상 백업 하나로 저장하는 방식을 승인했다. 최대 1,000개와 2MiB 상한을 유지한다. 전체 목표는 계속 진행 중이며 북마크 사용자 화면과 Preseller 내장은 아직 완료하지 않았다.

### 변경한 동작

- `src/bookmarks.rs`, `src/address.rs`, `src/lib.rs`: 추가, 이름 변경, 삭제, URL 중복 처리와 원자 교체 저장을 구현했다. 손상된 본문과 백업은 보존하고 읽기 전용으로 처리한다. 정상 빈 본문을 백업보다 우선해 삭제한 항목이 재로드 때 되살아나지 않게 했다. 파일 교체 후 디렉터리 동기화 실패는 반영된 변경과 경고로 구분한다. 아래 v6 바이너리에는 앱의 읽기, 비동기 쓰기와 AppKit 북마크 화면 연결이 포함되지 않았다.
- `src/tab_search.rs`, `src/control_panel.rs`, `src/browser.rs`: 같은 창에서 모든 공간의 열린 탭을 제목, URL, 공간 이름으로 검색한다. `⌘⇧A`, 방향키, Enter, Esc와 마우스를 지원한다. 입력 200자, 결과 40개로 제한하고 추가 웹 렌더러나 반복 타이머를 만들지 않았다. 결과 선택 때 현재 탭 소속을 다시 조회한다.
- 검색 중 뒤쪽 도구막대와 사이드바를 숨겨 키보드 포커스가 가려진 주소창으로 이동하지 않게 했다. 메뉴 닫기와 새로 고침이 검색 때문에 무시되는 경로도 고쳤다.
- 네이티브 검증에서 잘못된 주소를 제출하면 이전 URL이 입력을 덮는 문제를 발견했다. 실패한 입력을 보존하고, 편집을 끝낸 뒤 같은 탭에서 다른 탐색이 일어나면 초안과 이전 오류를 해제하도록 수정했다. 탐색 epoch와 URL 변경을 구분하는 회귀 검사도 추가했다.
- 검색 문자열에 URL 두 개가 붙어 있을 때 뒤쪽 URL의 자격증명을 놓치는 경로를 수정했다. `src/mac.rs`의 Objective-C 호출 인자 구문 경고도 제거했다. `tests/fixtures/user-control.html`에는 구분용 제목과 같은 탭 탐색 링크를 추가했다.
- `README.md`, `docs/product-plan.md`, `docs/bookmarks-design.md`에 실제 구현 범위, 승인 상태와 남은 앱 연결을 반영했다.

### 현재까지 확인한 결과

`artifacts/daily-browser-v6-rust-tests.log`에서 Rust 검사 117개가 통과했다. 북마크 모듈의 9개 검사에는 임시 파일 재로드, 손상 보존과 저장 실패 주입이 포함된다. 실제 앱 재시작, 전원 차단과 북마크 UI 검사로 해석하지 않는다. 초기 `openat` variadic 인자 컴파일 오류는 `libc::c_uint`로 수정했으며 실패 로그도 보존했다. 최종 `cargo fmt --check`와 공백 검사는 통과했다.

주소 보존 수정 전 바이너리 `84483d5c32751a88a043594a4725f9583ebd9bf76afc704fa29ec6ef12a35e1e`에서 Computer Use와 CLI 상태로 다음을 확인했다. 근거는 `artifacts/daily-browser-v4-native.json`이다.

- 새 탭의 주소 자동 포커스와 일반 한글 검색어의 Google 요청 URL 변환.
- 공간을 넘는 검색과 기존 탭으로 이동, 방향키 선택, 마우스 선택, 빈 결과에서 Enter.
- Tab, Shift+Tab 이후 검색 입력 유지와 검색 중 파일 메뉴의 탭 닫기.
- 배경에서 닫힌 탭의 결과 제거와 검색어, 포커스 유지.
- 위 검색과 사용자 탭 조작 중 두 AI 작업이 revision 1, active 상태를 유지함.

당시 로컬 문서 탐색은 키체인 승인 대기로 완료되지 않았다. Google 결과, 실제 본문 클릭과 키 입력의 자동 handoff, IME 조합은 이 검증에 포함되지 않는다. 한글은 붙여넣기로 입력했다. 오류 안내 뒤 입력이 바뀌는 문제를 발견한 결과와 수정 후 결과를 구분한다.

### v6 재검증 결과

주소 초안 보완을 포함한 v6 바이너리 SHA-256은 `7f855dcd1cfaf91523c631c5d6b083e87663d43d06608d07edc2a3e41710f546`이다. `artifacts/daily-browser-v6-native.json`에는 최초 로컬 DOM 대기 두 번의 `WAIT_TIMEOUT`도 남겼다. 각각 5초 대기 뒤 commit 전, 탐색 진행 중 상태였다. 키체인 승인 뒤 같은 바이너리에서 로컬 페이지가 열렸으며, 승인 전 실패를 성공 결과에서 삭제하거나 다른 바이너리 결과로 대체하지 않았다.

- 로컬 CEF 회귀: `artifacts/daily-browser-v6-suite.json`의 tasks 6개, spaces 10개, single_window 6개, 총 **3묶음 22개 검사**가 모두 통과했고 각 실행의 exit code는 0이다.
- 네이티브 주소와 검색: 잘못된 `javascript:alert(1)` 제출 뒤 입력과 오류를 보존하고 수정할 수 있도록 선택했다. `⌘T`로 새 탭을 열면 이전 오류가 해제됐다. 오류 후 본문 링크로 같은 탭을 탐색하면 실제 새 URL로 갱신되고 오류가 사라졌다. 검색 열기, 붙여넣기, Esc를 5회 반복해 이전 사용자 탭으로 복귀했고, 한글 제목 검색으로 기존 Beta 탭을 선택했다.
- 네이티브 사용자 개입: 좌표로 버튼 클릭, 입력칸 포커스 클릭, 이미 포커스된 입력칸의 `x` 키, 마우스의 편집 > 실행 취소, 0이 아닌 휠 입력을 각각 확인했다. resume 후 입력을 나눠 실행해 해당 Alpha 작업만 중지되고 Beta는 active를 유지하는 것을 확인했다. 이전 revision과 현재 paused scope는 읽기 요청을 거절했다.
- 페이지의 실제 기록: 클릭 결과는 `clicks=2`, `clickTrusted=true`였고, `x` 입력은 `inputTrusted=true`로 한 글자를 추가했다. 메뉴 실행 취소는 기존 입력값을 복원했으며 휠은 `scrollY=416`을 만들었다. CLI의 클릭, fill, scroll은 두 작업을 중지하지 않았다. 마지막 주소 편집과 검색 전환 뒤 Alpha는 active/revision 11, Beta는 active/revision 1을 유지했다. 위 네이티브 근거는 `artifacts/daily-browser-v6-native.json`의 12개 수기 확인 항목과 CLI 응답이다.

한글은 붙여넣기로 입력해 IME 조합은 미검증이다. 마우스 붙여넣기·다시 실행과 모든 편집 메뉴, 네이티브 입력 중 진행 중인 wait 취소, 실제 외부 사이트 호환성과 장시간 부하는 확인하지 않았다. 이번 결과를 새 모의 DOM 검사나 전체 외부 서비스 검증으로 해석하지 않는다.

후속 북마크 runtime과 AppKit UI 소스는 별도로 작성되어 Cargo 검증 중이다. 해당 연결은 v6에 포함되지 않았으므로 위 117개 Rust 검사, 22개 로컬 CEF 검사와 네이티브 결과로 북마크 UI, 비동기 저장, 앱 재시작 보존까지 통과했다고 보고하지 않는다.

### 빌드와 정리 기록

초기 v4 검증 작업 두 개와 생성한 탭을 모두 종료하고 기존 사용자 탭 1을 보존했다. 마지막 pending waits, 생성 예약과 캡처는 모두 0이었다. 해당 앱은 키체인 승인 대기 상태에서 정상 종료를 요청했으나 CEF teardown watchdog 메시지와 exit 2로 끝났다. 남은 이전 프로세스는 없었다. 원인은 이 로그만으로 확정하지 않는다.

첫 빌드는 중간 산출물 4,052개, 약 2.0GiB를 제거했다. 주소 초안 보완을 담은 중간 빌드는 1,263개, 약 668.5MiB를 정리했으며 앱을 실행하지 않았다. v6 release 빌드는 성공했고 `artifacts/daily-browser-v6-build.log`에 중간 산출물 3,536개, 약 1.8GiB 제거와 최신 release 실행 파일 보존이 기록됐다.

v6 검증 작업 Alpha와 Beta는 모두 finished, 소유 탭 0으로 정리했다. 테스트 사용자 탭도 닫고 기존 `about:blank` 사용자 탭 1만 남겼다. 마지막 pending waits, 생성 예약, 진행 중 캡처는 모두 0이다. 그 상태에서 `artifacts/daily-browser-v6-memory.json`의 3회 표본은 각각 6개 프로세스와 RSS 합계 **731.312MiB**를 기록했다. 검사 후 빈 탭 상태의 측정이며 새 실행의 초기 메모리나 고유 RAM 사용량이 아니다. 공유 매핑을 중복 계산할 수 있어 메모리 절감이나 누수 없음의 증거로 사용하지 않는다.

이후 v6는 `⌘Q`로 종료했고 실행 세션 18709에서 exit 0을 확인했다. `artifacts/daily-browser-v6-app.log`에는 teardown watchdog 메시지가 없다. v4의 exit 2와 구분되는 이번 정상 종료 관찰이며 이전 종료 문제의 원인 해결을 입증하지 않는다. 사용자 프로필은 보존했고 커밋, 푸시, 배포는 하지 않았다.

## 2026-10-11, 일상 사용성 소스 개선과 Aside·ego lite 실제 앱 비교

활성 목표는 Naru를 먼저 완성하고 Aside의 일상 사용성과 ego lite의 사용자·AI 공간 및 CLI를 결합하는 것이다. 사용자는 세로 탭·검색·북마크를 우선하도록 선택한 뒤 Aside를 설치하고 Computer Use 비교를 요청했다. 진행 중이던 Naru 소스 개선을 단위 검사까지 정리하고 실제 앱 비교로 작업을 전환했다. 전체 목표는 계속 진행 중이며 배포나 Preseller 내장 구현을 완료한 상태가 아니다.

### 작성한 소스와 현재 실행 상태

- `src/address.rs`, `src/lib.rs`: 주소창 입력을 HTTP(S) URL, 로컬 주소, 일반 Google 검색어로 구분한다. 빈 입력, 지원하지 않는 스킴, 길이 상한, 자격증명이 들어 있는 주소를 처리한다. 검색 연산자 뒤에 들어 있는 자격증명 URL도 검색 서비스로 보내지 않도록 검사했다. CLI의 URL 정책은 바꾸지 않았다.
- `src/mac.rs`, `src/control_panel.rs`, `src/browser.rs`: 현재 표시된 CEF 본문의 클릭·키 입력·휠을 AppKit에서 확인해 해당 작업의 기존 handoff 경로로 연결했다. 본문 first responder를 대상으로 하는 편집 메뉴도 `sendAction:to:from:`에서 감지한다. 주소창, 공간 목록, 숨은 뷰는 제외하고 원래 이벤트와 메뉴 동작은 계속 전달하도록 했다. 이 경로는 OS 자동화 입력도 감지하므로 실제 물리 장치 입력만을 식별하는 장치는 아니다.
- 새 탭 생성 뒤 주소창 포커스, 빈 주소의 안내 문구, 선택 탭의 접근성 이름, 오류 안내의 전체 도움말을 보완했다. `tests/fixtures/user-control.html`에 자체 클릭·입력·스크롤 기록 페이지를 만들었다.
- 독립 검토에서 검색어에 섞인 자격증명 URL과 마우스로 여는 편집 메뉴의 감지 누락을 찾아 수정했다. 최종 `cargo test --locked --offline --lib`는 **102개 통과, 실패 0개**다. 근거는 `artifacts/daily-browser-rust-tests.log`다. `src/mac.rs`의 `msg_send!` 인자 쉼표 형식에 대한 deprecation 경고 한 건이 남아 있다.
- 초기 컴파일의 `event.window` 인자 및 `superview()` unsafe 호출 오류는 수정했다. 새 소스로 release 빌드, CEF 회귀, 실제 Naru 창에서 주소 검색·직접 입력·메뉴 handoff 검증은 아직 하지 않았다. 현재 `dist`의 앱은 이전 UI 다듬기 버전이며 위 기능이 실행 중이라고 보고하지 않는다.

### 실제 앱 비교 결과

상세 기록은 [Aside와 ego lite 실제 앱 비교](browser-comparison-2026-10-11.md)에 있다. 사용자 요청에 따라 설치 앱은 native Computer Use로 조작했고, ego lite의 외부 에이전트 제어만 ego-browser CLI로 실행했다. 앱 표시 버전은 Aside `1.0.928.1`, ego lite `0.5.1.13`이다.

- ego lite: 테스트 사용자 공간과 두 탭, 주소 검색, 북마크 이름 수정·재열기, 세로 탭 접기·복원을 확인했다. 마지막 테스트 탭을 닫으면 해당 공간도 사라졌다. 별도 CLI 작업 공간 8은 기존 사용자 공간의 선택을 유지했고 `Running` 카드와 `Agent is in control`, `Take over`, `Stop`을 표시했다.
- Aside: Bookmarks/Chats/Tabs 세로 사이드바, 새 탭 중앙 Search/Ask와 초기 입력 포커스, 북마크 이름 수정·재열기, 사이드바 접기·복원을 확인했다. 북마크로 기존 페이지를 다시 선택했을 때 클릭·입력 상태가 유지됐다. 통합 검색에서 열린 탭·북마크·기록 결과를 확인했다.
- 두 앱 모두 자체 로컬 페이지에서 에이전트 경로로 클릭 한 번과 입력 한 번을 수행했다. 실제 페이지가 `clicks=1`, `inputEvents=1`, 각 이벤트의 `isTrusted=true`를 기록했다. Aside는 현재 페이지 옆 AI 패널에서 수행했고, ego lite는 CLI 작업 공간에서 수행했다.
- Aside에 별도 반복 요청을 보낸 직후 Stop을 누르자 `Interrupted`, `Worked 0s`로 바뀌었다. 이후 재확인에서도 페이지 값은 1회·1회였다. 클릭 실행 도중 취소, 이미 전달된 명령의 취소, 사용자 본문 입력에 따른 자동 인계는 검증하지 않았다. ego lite의 Take over·Stop은 표시만 확인했다.
- Aside 중앙 검색은 Google 요청 뒤 `/sorry`로 이동해 결과 확인을 마치지 못했다. CAPTCHA를 해결하거나 차단을 우회하지 않았다. ego lite와 프로필·요청 조건이 달라 이를 앱 결함이나 검색 성능 차이로 단정하지 않는다.

이 결과를 바탕으로 `docs/product-plan.md`에 일상 사용 동선은 Aside, 작업 공간·제어권·CLI는 ego lite를 참고하는 방향을 반영했다. 북마크와 통합 검색은 아직 Naru에 구현하지 않았다. 북마크의 영속 저장 방식도 결정하지 않았다. `README.md`, `AGENTS.md`에는 개발 계획과 합의한 우선순위를 연결했다. UI용 웹 renderer 없이 기존 AppKit 단일 창을 유지하는 방향이다.

### 검증 한계와 정리

마지막 `cargo fmt --check`와 `git diff --check`가 통과했다. 두 참고 앱의 메모리를 동일 조건으로 측정하지 않았으며 어느 쪽이 더 가볍다는 결론을 내리지 않았다. 로그인·파일·iframe 전체 호환성, 장시간 부하, 모든 크기·다크 모드·전체 접근성 품질도 이번 비교 범위 밖이다. 참고 앱의 성공과 Naru 새 소스의 성공은 별개다.

ego lite의 임시 사용자 북마크와 테스트 공간을 정리했다. CLI TaskSpace 8은 `finish({keep: []})`를 한 번 호출해 종료했고 처음의 두 공간만 남은 것을 확인했다. 이전 공식 사이트 조사용 TaskSpace 6도 이미 종료된 상태다. Aside의 임시 북마크와 테스트 탭 세 개를 닫았고, 기존 New Tab과 도중에 열린 확장 온보딩 탭을 보존했다. 결과 확인용 대화 `브라우저 UI clicks 및 inputEvents 검증` 한 개는 남겼다. 프로필·쿠키·기존 사용자 탭은 삭제하지 않았다.

로컬 HTTP 서버 19949를 종료했다. `scripts/cleanup-build.py --profile release`로 Cargo 중간 산출물을 정리하고 기존 release 실행 파일 세 개를 보존했다. Cargo는 2,794개 파일, 1.3GiB 제거를 보고했다. 정리 후 `target`은 약 2.4MiB, 기존 `dist`는 약 335MiB다. 새 Naru 앱 프로세스나 번들은 만들지 않았다. 커밋·푸시·배포는 하지 않았다.

## 2026-10-11, Naru 독립 완성 후 Preseller 내장으로 방향 정정

사용자는 Naru 자체를 ego lite처럼 완성하고 그 Naru를 Preseller 안에 내장하는 순서를 명확히 했다. 앞선 적합성 검토에서 제안한 Electron 기반 연결 우선 방향은 이 목적과 달랐으므로 철회했다. 아래 이전 기록은 당시 검토 이력이며 현재 개발 방향은 이 항목과 갱신된 설계 문서를 따른다.

- `docs/preseller-integration.md`를 독립 브라우저 완성 → 같은 구현의 코어·호스트 경계 분리 → Preseller 내장 순서로 수정했다. 일반 사용, 사용자·AI 협업, 관찰·입력, 복구, 자원 관리의 완료 기준을 추가했다.
- 독립 창과 내장 화면이 동일한 브라우저 기능을 사용하도록 하는 책임 분리를 제안했다. 부모 뷰·포커스·팝업·종료, 프로필·파일·권한, 공통 명령·이벤트의 소유 주체와 남은 기술 검증을 명시했다. 이 경계는 아직 구현되지 않았다.
- `README.md`, `docs/architecture.md`, `AGENTS.md`에 사용자가 확정한 목표와 문서 연결을 반영했다. Preseller 런타임의 변경이나 Naru 내장 방식은 이번에 확정하지 않았다.
- 문서의 방향 일치와 구현·계획의 구분을 검토하고 수정 파일의 공백 검사를 실행했다. 소스 코드 변경과 앱·테스트·빌드 실행은 하지 않았다. 문서 수정이므로 생성한 프로세스나 빌드 산출물은 없으며, 커밋·푸시·배포도 하지 않았다.

## 2026-10-11, Preseller 내장 브라우저 적합성 검토

사용자가 Naru를 Preseller 화면 안의 내장 브라우저로 사용하려는 목적을 확인했다. 참조 대화 `preseller-app 완성 설계 및 구현`의 최근 기록을 읽고, Naru의 앱 시작 → 자체 창 생성 → CEF 뷰 생성 → 로컬 소켓 명령 처리 경로를 정적으로 검토했다.

- `docs/preseller-integration.md`에 현재 재사용 가능한 기능, 호스트 연결의 누락, 런타임 선택의 영향, 상품 한 건을 수집·저장하는 첫 검증 범위를 기록했다.
- 현재 Naru는 자체 창과 CEF 수명주기를 소유하며 외부 앱에 끼워 넣는 API가 없다. 작업·공간·CLI 계약은 기반이 되지만 프로필 분리, 호스트 이벤트와 상품 수집 결과 연결은 별도 작업이다.
- 참조 대화의 최근 기록은 Electron 기반이다. 이를 유지할 경우 기존 Chromium을 사용하는 내장 뷰에 Naru의 제어 규약을 적용하는 방향을 제안했다. Rust/CEF로의 호스트 전환이나 Preseller 구현 변경을 결정·실행하지 않았다.
- 검증은 Naru 소스와 참조 대화의 읽기, 문서의 `git diff --check`로 제한했다. Preseller 소스, 실제 사이트 수집, 연결 동작, 메모리 절감은 검증하지 않았다. 기존 Naru 로컬 회귀 결과와 Preseller 통합 완료를 구분했다.
- 소스 코드 변경, 새 빌드·앱 실행, 커밋·푸시·배포는 없다. 이번 검토에서 생성한 실행 프로세스나 빌드 산출물이 없어 추가 정리는 필요하지 않았다.

## 2026-10-11, 브라우저 UI 다듬기

투박한 화면을 다듬어 달라는 요청에 맞춰 단일 AppKit 창의 시각 요소와 배치를 정리했다. 새 기능이나 별도 작업창을 추가하는 범위는 아니다.

### 변경한 화면과 파일

- `src/control_panel.rs`: 중성 배경과 청록색 선택 표시, 둥근 주소 입력 영역, 작은 작업 상태 배지, 일정한 시스템 아이콘을 적용했다. 44pt 탐색 막대와 36pt 탭 행의 여백을 정렬하고 탭 아이콘·제목·클릭 영역을 분리했다. 아이콘 버튼의 접근성 이름과 도움말을 유지했다.
- 공간 카드는 넓은 미리보기 아래에 제목, 탭 정보, 상태와 중지를 모았다. 너비에 따라 1~3열로 배치한다. 기존 NSImage를 상단 중앙 기준으로 비율을 유지해 채우고 마스크로 자르며 별도 확대 이미지를 만들지 않는다.
- SF Symbols는 고정된 종류를 재사용한다. 별도 UI renderer, 외부 이미지 파일, 상주 타이머를 추가하지 않았다. 라이트·다크 색상을 구분하는 네이티브 appearance 처리를 구현했다.
- `Cargo.toml`: 기존 AppKit 의존성에 `NSAppearance`, `NSCell`, `NSFontDescriptor` 기능을 추가했다. 이 변경 때문에 새 런타임 프로세스나 패키지를 추가하지 않았다.
- 중지 버튼의 도움말을 실제 작업 범위에 맞춰 ‘현재 공간의 AI 작업 중지’로 정정했다. `README.md`, `docs/architecture.md`, `docs/memory.md`에도 현재 구성을 반영했다.

### 검증 결과

최종 바이너리 SHA-256: `b35a9f4e7c5e2237d4c35af77f98578d1a397c977e9853dd99c576d9febb7a44`.

종합 근거는 `artifacts/ui-polish-validation.json`이다.

| 수준 | 실행 결과 | 근거 |
| --- | --- | --- |
| Rust 단위 검사 | 88개 통과 | `artifacts/ui-polish-rust-tests.log` |
| macOS release 빌드 | 성공, 앱 번들 교체 | `artifacts/ui-polish-build.log` |
| 로컬 CEF 공간 회귀 | 10항목 통과 | `artifacts/ui-polish-spaces-smoke.json` |
| 로컬 CEF 단일 창 회귀 | 6항목 통과 | `artifacts/ui-polish-single-window-smoke.json` |
| 네이티브 화면·입력 | 종합 보고서의 수기 10항목 확인 | 탭 제목/아이콘 클릭, 사이드바, 주소 선택, 카드, 공간 열기, 중지, 좁은 창, 재실행 |
| 정적 검사 | `cargo fmt --check`, `git diff --check` 통과 | 도움말 수정 뒤 포맷을 정리했으며 별도 검토에서 P1/P2 차단 결함 없음 |

라이트 모드의 실제 화면에서 사이드바 배경, 탭 안쪽 여백, 아이콘, 주소창과 배지가 표시됐다. 탭 제목과 아이콘의 좌표를 각각 클릭해 페이지와 주소 전환을 확인했다. ⌘⇧B로 사이드바를 접고 복원했으며 ⌘L이 주소를 선택했다. 세 개의 로컬 페이지 미리보기가 카드 영역을 채우고 다른 페이지임을 구분할 수 있었다. 800pt 너비의 좁은 창에서는 카드가 한 열로 바뀌고 스크롤됐으며 사용자·AI 탐색 막대가 겹치지 않았다. 실제 창 높이는 800pt 상태였고 정확한 800×520 최소 크기는 정적 배치 계산만 확인했다.

상단 중지는 해당 작업을 paused로 바꾸고 revision을 1에서 2로 올렸다. 이전 revision의 명령은 `TASK_STALE_REVISION`으로 거절됐고 다른 작업은 active 상태를 유지했다. 이어 다른 공간의 카드 중지도 paused와 revision 2를 만들고 중지 버튼을 숨겼다. CLI 응답은 `artifacts/ui-polish-after-session.json`에 기록했다.

전후 화면은 `artifacts/ui-polish-before-overview.jpg`, `artifacts/ui-polish-after-overview.jpg`에 있다. 새 탐색 화면과 좁은 창은 `ui-polish-after-browsing.jpg`, `ui-polish-after-narrow-overview.jpg`, `ui-polish-after-narrow-ai.jpg`다. 모두 자체 로컬 검증 페이지이며 외부 서비스 화면이 아니다.

### 승인 대기와 남은 제약

- 새 ad-hoc 서명으로 실행한 첫 로컬 탐색은 commit 전에 `WAIT_TIMEOUT`이었다. 사용자가 키체인을 ‘항상 허용’한 뒤 같은 바이너리에서 페이지가 열렸다. 승인 전 실패는 `artifacts/ui-polish-setup.log`에 남겼다.
- 승인 대기 중 시작한 미리보기 요청 하나는 승인 후에도 진행 중으로 남았다. 해당 검증 탭만 닫아 자리를 반환하고, 승인 후 새로 만든 세 페이지에서는 미리보기가 표시됐다. 이 경로의 자동 복구는 아직 수정하지 않았으며 후속 과제다. `artifacts/ui-polish-approval-session.json`에 해당 검증 탭과 정리를 구분해 기록했다.
- 다크 색상은 구현·정적 확인했지만 macOS의 사용자 설정을 바꾸지 않아 실제 다크 전환은 검증하지 않았다. 외부 사이트·로그인, 8개 카드 부하, 디코딩 할당량과 physical footprint도 이번 범위에서 확인하지 않았다. DOM·Python 격리 검사는 UI만 변경한 이번 작업에서 다시 실행하지 않았다.
- 미리보기 해상도 상한은 유지한다. 한 열이나 최대화된 큰 카드에서는 이미지가 부드럽게 보일 수 있다. 이 작업을 메모리 절감이나 브라우저 전체 완성으로 해석하지 않는다.

### 메모리와 정리

같은 바이너리를 정상 종료·재실행하고 로컬 회귀를 마친 뒤 빈 사용자 탭 하나, 공간 목록 닫힘 상태에서 3회 측정했다. 6개 프로세스의 RSS 합계 최고값은 691.062MiB, 본체는 237.078MiB였다. 공유 매핑이 중복되는 합계이며 고유 RAM이나 이전 대비 절감량이 아니다. 근거는 `artifacts/ui-polish-final-memory.json`이다. 최초 `ui-polish-after-memory.json`은 미리보기 대기 중에 측정됐으므로 전후 대조 자료로 쓰지 않는다.

빌드 과정에서 이전 앱 PID 17631을 종료·교체하고 중간 파일 1,269개, 약 669.7MiB를 제거했다. 화면 검증용 앱 PID 33342도 정상 종료했으며 최종 메인 PID는 34161 하나다. `dist`에는 최신 앱 번들 하나, `target/release`에는 실행 파일 세 개가 남았다. `target` 약 2.4MiB, `dist` 약 335MiB, CEF 캐시 약 339MiB다.

이번 작업의 로컬 HTTP 서버와 검증용 탭·작업을 정리하고 기본 창 크기로 다시 열었다. 기존 사용자 탭 1과 프로필을 보존했다. 마지막 `pendingWaits`, `pendingTabCreations`, `capturesInFlight`는 모두 0이다. 커밋·푸시·배포는 하지 않았다.

## 2026-10-10, 사용자 공간과 AI 작업 공간 분리

요청한 범위의 구현과 로컬 검증을 마쳤다. `/goal`에 등록한 목표는 단일 브라우저 창 안에서 사용자 공간과 CLI 작업별 공간을 나누고, 공간 전환·작업 중지·가벼운 네이티브 UI를 연결하는 것이었다. 배포 준비 전체를 완료한 기록은 아니다.

### 변경한 동작과 파일

- `src/spaces.rs`: 공간별 탭과 선택, 빌린 탭의 복귀 위치를 관리한다. AI 작업 생성과 배경 탭 생성은 현재 사용자 화면을 바꾸지 않는다. `finish --keep` 결과는 완료 공간에 남기고 borrowed 탭은 원래 공간으로 돌려보낸다.
- `src/browser.rs`: 생성·팝업·탭 종료·handoff에 공간 모델을 연결했다. 숨겨진 탭의 포커스 요청을 거절하고 공간 선택은 명시적인 UI/CLI 전환에서만 바꾼다. 활성 AI 팝업은 같은 공간과 작업 소유권을 유지한다.
- `src/control_panel.rs`: AppKit 창 하나에 접는 세로 탭, 공간 카드, 주소창, 작업 상태와 중지를 구현했다. `SurfaceView.updateLayer`로 배경과 얇은 경계선을 유지한다. 별도 작업창이나 UI 웹 renderer는 추가하지 않았다.
- `src/cli.rs`, `src/protocol.rs`: `spaces`, `space show user|TASK_ID`, `space overview`를 추가했다. 작업 범위 명령은 사용자의 전역 공간 전환을 실행할 수 없다.
- 공간 미리보기는 현재 목록의 최대 8개 카드만 대상으로 하며 CLI 캡처와 자리 하나를 공유한다. PNG 수신 크기와 픽셀 상한을 검사하고 목록을 닫으면 UI 이미지 참조를 해제한다. 빈 탭 캡처, 반복 polling, 디스크 이미지 캐시는 없다.
- `tests/spaces_smoke.py`: 배경 생성의 선택 보존, 공간 전환, 빈 공간, adopt/release, 완료 결과 보존과 정리를 검사한다. `tests/single_window_smoke.py`는 명시적 공간 전환에 맞췄다. 팝업 배치와 늦은 미리보기 응답에 단위 회귀 검사를 추가했다.
- `Cargo.toml`, `Cargo.lock`: 네이티브 글꼴·이미지·스크롤 UI에 필요한 AppKit 기능을 연결했다. 런타임용 Node 프로세스는 없다.
- `README.md`, `docs/architecture.md`, `docs/memory.md`에 설계·사용법·검증·메모리 조건을 갱신했다. `AGENTS.md`에는 작업을 완료했다고 보고하기 전에 이 기록을 갱신하는 규칙을 추가했다.

### 최종 검증

앱 바이너리 SHA-256: `710ec71ecfcb0fa374d875eba0ad24492b3a6d63d603e8a0da455bac0d7a8cca`.

종합 근거: `artifacts/spaces-v3-validation.json`.

| 검증 수준 | 결과 | 근거 |
| --- | --- | --- |
| Rust 단위 검사 | 88개 통과 | `artifacts/spaces-v3-rust-tests.log`; 최종 스타일 수정 포함 |
| 모의 DOM 검사 | 20개 통과 | `artifacts/spaces-v1-dom-tests.log`; 이후 DOM 소스 변경 없음 |
| Python 격리 검사 | 22개 통과 | `artifacts/spaces-v1-python-tests.log`; 이후 해당 구현 변경 없음 |
| 실제 CEF, 로컬 HTTP | 8개 묶음, 54항목 통과 | 기존 7묶음 44항목은 `artifacts/spaces-v3-suite.json`, 새 공간 10항목은 `artifacts/spaces-v3-spaces-smoke-after-allow.json` |
| 네이티브 UI와 CLI 재확인 | 정리를 포함한 수기 검사 16항목 | `artifacts/spaces-v3-native.json`; 아래 범위로 제한 |
| 정적 검토 | 공간·팝업·handoff·미리보기 자원 흐름 검토, `git diff --check` 통과 | 실행 검증과 구분 |

네이티브 검사에서는 사용자 페이지를 유지한 채 AI 두 공간의 배경 읽기가 성공했다. 실제 세 페이지의 서로 다른 썸네일이 표시됐고, 목록을 연 상태에서 배경 페이지를 이동하면 해당 카드 이미지와 제목이 갱신됐다. 공간을 오갈 때 사용자 페이지와 AI 공간에서 선택한 두 번째 탭이 각각 복원됐다.

실제 페이지 버튼의 `window.open`은 원래 URL과 선택을 유지하면서 같은 작업 공간에 소유권이 있는 탭을 만들었다. 빈 공간과 페이지가 있는 공간의 중지를 각각 눌러 paused 상태, revision 1→2, 이전 revision 거절과 다른 작업 유지를 확인했다. 접는 사이드바, ⌘⇧S/⌘⇧B, ⌘T/⌘L/⌘W도 확인했다. ⌘L 검사에서 주소 문자열은 바꿨지만 탐색은 제출하지 않았다.

화면 근거는 `artifacts/spaces-v3-overview-pages.jpg`, `artifacts/spaces-v3-overview-updated.jpg`, `artifacts/spaces-v3-user-during-ai.jpg` 등에 보존했다. 이 이미지는 로컬 검증 페이지이며 외부 서비스 결과가 아니다.

### 발견한 문제와 처리

- 최초 제한 환경의 Rust 소켓 검사에서 `Operation not permitted`가 발생했다. 로컬 소켓 접근 권한을 조정한 최종 검사에서 88개가 통과했다.
- 초기 앱 실행의 로컬 페이지는 commit 전에 `WAIT_TIMEOUT`으로 멈췄다. 사용자가 키체인의 ‘항상 허용’을 누른 뒤 재빌드 없이 같은 최종 바이너리에서 공간 검사와 전체 회귀가 통과했다. 승인 전 실패는 `artifacts/spaces-v3-spaces-smoke.json`에 보존했다. 이후 재서명에서도 무승인 실행된다는 뜻은 아니다.
- 처음 네이티브 화면에서 카드와 사이드바 배경이 유지되지 않았다. plain backing layer 직접 변경을 `SurfaceView.updateLayer`로 바꾸고 실제 화면의 경계를 확인했다.
- 코드 검토에서 다른 카드 갱신 때문에 늦은 미리보기 결과가 폐기될 때 재요청이 막히는 경우와 팝업의 background disposition 반영 누락을 발견했다. 두 경로를 수정하고 단위 회귀 검사를 추가했다.

### 메모리와 정리

전체 검사 후 빈 사용자 탭 하나, 공간 목록 닫힘 상태에서 6개 프로세스를 1초 간격으로 3회 측정했다. RSS 합계 최고값은 710.453MiB, 본체 RSS는 247.859MiB였다. 공유 매핑이 중복되는 합계이며 고유 RAM 사용량이나 이전 버전 대비 절감량이 아니다. 페이지 세 개와 빈 탭, 미리보기 세 개를 표시한 별도 조건의 합계는 1,073.859MiB였다. 조건과 해석은 `docs/memory.md`, 원본은 `artifacts/spaces-v3-memory-after-tests.json`과 `artifacts/spaces-v3-memory-overview.json`에 있다.

각 빌드에서 이 프로젝트의 이전 앱을 종료하고 새 번들로 교체했으며 Cargo 중간 산출물을 정리했다. 마지막 확인에서 앱 메인 PID는 17631 하나였고 `dist`에는 최신 `Light Browser.app` 하나, `target/release`에는 실행 파일 세 개만 남았다. 디스크 사용량은 `target` 약 2.4MiB, `dist` 약 335MiB, CEF 캐시 약 339MiB, 검증 자료 약 7.7MiB다.

검증용 HTTP 서버를 종료하고 생성한 작업·탭을 정리했다. 기존 사용자 탭 1과 `.local-data/tasks-phase1/profile-v1`은 보존했다. 최종 생성 예약과 진행 중 캡처는 모두 0이다. 커밋·푸시·배포는 하지 않았다.

### 남은 제약과 후속 범위

- 공간은 표시와 선택을 나누며 프로필·쿠키는 공유한다.
- 페이지 본문 직접 입력 시 자동 handoff는 아직 없다. AI 작업은 중지 후 직접 조작한다.
- 숨긴 탭을 자동 폐기하지 않으며 Chromium 전체 RAM 상한은 없다.
- 실제 미리보기 PNG 픽셀 치수·디코딩 할당량, 8개 카드 페이지 부하, 최종 버전의 마지막 탭 종료와 beforeunload 취소는 이번 실행에서 검증하지 않았다.
- 외부 사이트 로그인·전체 호환성, 안정적인 코드 서명과 공증, 배포는 별도 작업이다.

## 검증 전용 mock keychain 모드

### 요청 범위와 변경

ad hoc 서명 빌드를 키체인 재승인 없이 검증 실행할 수 있도록 `NARU_MOCK_KEYCHAIN=1` 모드를 추가했다. `command_line_args_disabled`로 명령행 스위치를 줄 수 없어 `BrowserApp`의 `on_before_command_line_processing`에서 브라우저 프로세스에만 `use-mock-keychain`을 붙인다. `LIGHT_BROWSER_DATA_DIR` 없이 `NARU_MOCK_KEYCHAIN=1`이면 `main`이 시작 전에 오류로 종료한다. 실제 프로필이 mock 키로 열려 기존 암호화 쿠키를 읽지 못하게 되는 일을 막기 위해서다. 관련 파일은 `src/config.rs`(판정 함수와 단위 테스트), `src/main.rs`, `src/browser.rs`, `scripts/dev.sh`(두 변수가 모두 있으면 ad hoc `--run` 허용, 도움말 갱신), `README.md`다. `dev.sh`는 이미 번들 바이너리를 `exec`하므로 환경 변수가 그대로 전달된다.

### 검증

| 구분 | 내용 | 결과 |
|---|---|---|
| 단위 테스트 | `cargo test --locked --lib mock_keychain` | 1개 통과 |
| 빌드 검사 | `cargo check --locked` | 통과, 기존 dead_code 경고 1건 |
| 스크립트 검사 | `bash -n`, `--help`, 환경 변수 없는 `--run` 거부(exit 2) | 확인 |
| 릴리스 번들 | `./scripts/dev.sh --release --build-only` | 성공, 약 45초, 바이너리에 `use-mock-keychain` 문자열 포함 확인 |

### 한계와 남은 작업

- E2E v11부터 v14까지 이 모드로 앱을 실행했고 키체인 프롬프트 없이 시작했다. `NARU_MOCK_KEYCHAIN=1` 단독 실행의 거부 메시지와 두 변수를 준 `--run` 경로는 이 실행에서 직접 확인하지 않았다.
- 스위치는 browser process에만 붙인다. 헬퍼 프로세스는 이 훅을 거치지 않는다.
- mock 키로 만든 프로필은 실제 키체인 프로필과 호환되지 않는다. 검증 후 별도 데이터 경로를 그대로 둘지 지울지는 사용자가 정한다.
- 빌드 후 `cleanup-build.py`가 Cargo 중간 산출물(5258개, 3.1GiB)을 제거했고 최신 릴리스 실행 파일은 보존했다. 사용자 데이터와 키체인은 건드리지 않았다. 실행 중인 앱 프로세스는 없다. 커밋·푸시는 하지 않았다.

## CEF 브라우저 동일성 비교 수정

### 증상과 원인

375e41a 이후 빌드에서 `read TAB`은 45초 뒤 `TIMEOUT`으로 끝났다. `open`은 매번 `PAGE_CHANGED`와 `stateUnavailable:true`를 반환했다. 375e41a가 브라우저 동일성을 `get_raw()` 포인터 비교로 바꿨는데, CEF는 API 경계를 넘을 때마다 같은 브라우저를 새 C 구조체로 감싼다. 그래서 `on_after_created`에서 저장한 핸들과 `create_browser_sync`나 콜백에서 받은 핸들이 같은 브라우저인데도 다르다고 판정됐다. 샌드박스 밖 실행과 창 앞 배치에서도 같은 증상이었다.

### 변경

`src/browser.rs`에 `same_browser`를 추가했다. 포인터가 같으면 바로 같다고 보고, 아니면 CEF `is_same`으로 비교한다. Browser 포인터 비교 30여 곳을 모두 이 함수로 바꿨다. Frame은 `identifier()` 문자열로 비교하고 있어서 바꾸지 않았다. 리뷰에서 근거, 누락 여부, UI 스레드 호출 조건을 확인했고 승인을 받았다.

### 검증

| 구분 | 내용 | 결과 |
|---|---|---|
| 로컬 CEF 통합 | 수정 전 빌드에서 `read 1`, `open https://example.com` | 실패 재현, `TIMEOUT`과 `PAGE_CHANGED` |
| 로컬 CEF 통합 | 수정 빌드(앱 SHA-256 `6c422b23…`)에서 `read 1`, 사용자 `open`, 작업의 `open`, `wait load`, `read`, `task finish` | 통과, 작업 실행 전후 사용자 선택 `user 2` 유지 |
| 단위 테스트 | 실행하지 않음 | 다음 통합 회귀에서 실행 예정 |

검증 프로필은 `.local-data/e2e-v11`이다. 실제 사용자 프로필은 건드리지 않았다. 이 수정 뒤의 E2E 실행 결과는 2026-10-18 `e2e-v14` 기록에 있다.
