# Naru 작업 기록

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
