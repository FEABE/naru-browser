# 페이지 내 찾기

기준일: 2026-10-11. 페이지 찾기 v1에서 AI 작업 재개 후 이전 찾기가 남는 결함을 발견했다. 수정한 v2는 Rust 단위 검사 137개와 release 빌드를 통과했고, 사용자 승인 후 같은 바이너리에서 실제 재개·강조 정리·다른 작업 보존을 확인했다. v2의 작업·공간·단일 창 CEF 회귀 22개도 통과했다. 아래 v1과 v2 실행 결과를 구분한다.

## 사용 흐름

주소줄 아래에 네이티브 찾기 막대 하나를 표시한다. 본문을 계속 볼 수 있고 검색어, 현재 위치와 전체 결과 수, 이전 결과, 다음 결과, 닫기를 제공한다. 검색은 CEF의 기본 페이지 검색을 사용한다. 페이지 텍스트를 복사해 별도 색인이나 DOM 강조 표시를 만들지 않는다.

- `⌘F`는 현재 페이지의 찾기 입력을 선택한다.
- 검색어를 입력하면 CEF의 현재 위치를 기준으로 일치 항목의 선택과 스크롤을 요청한다. 최초 선택이 항상 문서의 1번 결과인 것은 아니다.
- `Enter`와 `⇧Enter`는 찾기 입력에서 다음 결과와 이전 결과로 이동한다.
- `⌘G`와 `⌘⇧G`도 다음 결과와 이전 결과로 이동한다.
- `Esc`는 막대를 닫고 강조를 해제한다. 같은 문서에서는 검색어 하나를 메모리에 남겨 다시 찾기에 사용한다.
- 탭 변경, 새 문서 탐색, 탭 종료, 공간 목록, 통합 검색이나 북마크 화면 전환은 찾기를 종료하고 이전 문서의 검색어를 지운다.

검색어는 최대 200자이며 제어 문자는 거절한다. 찾기 입력에서 조합 중에는 검색과 길이 보정을 유보한다. 찾기 단축키는 실제 창의 first responder가 `hasMarkedText`를 지원하면 조합 여부를 확인하므로, 본문·주소·찾기 입력의 조합 중 열기·다음·이전·Esc 처리를 가로채지 않는다. 이는 소스의 보호 경로이며 실제 한글 IME 조합 검증은 남아 있다. 결과 수집 중, 일치 항목 없음과 검색 실패를 구분한다.

## 사용자와 AI의 제어권

찾기는 결과 위치로 페이지를 스크롤할 수 있고 막대를 표시하면 본문 크기도 변한다. 따라서 AI 작업이 활성인 탭에서 사용자가 찾기를 열면 기존 `handoff_page_input` 흐름으로 해당 작업을 중지한다. 다른 AI 공간과 사용자 탭의 상태는 유지한다. 중지 전 revision으로 보낸 명령과 대기 요청은 기존 작업 규칙을 따른다.

결과 콜백과 찾아보기 명령은 현재 탭, 문서 탐색 세대와 요청을 대조한다. 늦게 도착한 이전 검색 결과를 현재 검색어의 결과로 표시하지 않는다. `begin_page_find`는 handoff 뒤의 탐색 세대를 반환하고 UI는 반환값을 사용한다. handoff만으로 세대가 바뀔 때 기존 주소 입력 초안을 버리지 않도록 기준 세대도 갱신한다. CEF와 재진입 가능한 AppKit 호출은 `SHELL`이나 `STATE`의 가변 borrow 밖에서 실행한다.

입력 중 상태 갱신은 찾기 필드의 문자열이나 포커스를 다시 설정하지 않는다. CEF의 포커스 요청은 찾기 편집기가 활성일 때 막는다. 실제 본문 mouse-down이 표시된 CEF 뷰의 hit test를 통과하면 편집기를 먼저 끝내고 원래 입력과 기존 handoff 흐름을 전달한다. 찾기 막대는 유지한다. v1의 네이티브 본문 클릭·입력에서 실제 focus·input 이벤트와 `isTrusted:true`를 확인했으며 IME 조합의 검증과는 구분한다.

## 메모리와 저장

찾기 막대, 제한된 검색어와 결과 메타데이터를 보관한다. 실행 중인 CEF 요청이 있을 때 같은 검색어의 다음·이전 요청은 가장 최근 한 건만 대기시키며, 뒤의 요청이 오면 대기 항목을 교체한다. 입력 횟수만큼 큐를 늘리지 않는다. UI용 웹 렌더러, 상주 worker, 파일 저장, 검색 색인과 반복 polling은 추가하지 않는다. 검색을 닫거나 대상 문서가 사라지면 관련 요청과 강조 상태를 정리한다. 이 구조만으로 Chromium 전체 메모리 절감을 주장하지 않는다.

## 소스 검증

`src/page_find.rs`는 CEF 식별자와 UI 요청 token을 연결하고 현재 요청·최대 한 건의 대기 요청을 관리한다. `src/browser.rs`는 표시 탭·문서 세대·준비 상태와 handoff를 확인한 뒤 CEF를 호출한다. `src/control_panel.rs`는 AppKit 막대, 단축키·포커스·조합 보호와 결과 표시를 담당한다.

`artifacts/find-page-v1-rust-tests.log`에서 전체 136개 단위 검사가 통과했고 `artifacts/find-page-v1-check.log`에서 `cargo check --all-targets`가 통과했다. 페이지 찾기의 순수 상태 회귀 8개는 검색어 교체, 대기 한 건으로 병합, 이전 식별자·문서·token 거절, 닫기·재열기, 미정 부분 결과, 입력 상한과 잘못된 요청, CEF 발행 전 결과 거절을 확인한다. 포맷과 `git diff --check`도 통과했다. 이 단위 결과와 아래 실제 실행 결과는 검증 수준이 다르다.

## v1 로컬 실행 결과

검증 바이너리의 SHA-256은 `0c0659c2c476f9a88a57f21ca9911baf361004ddea0f97693e95b38fbb11625d`이며 근거는 `artifacts/find-page-v1-native.json`이다. 페이지와 입력은 검증용 로컬 fixture를 사용했다.

- 창을 실제 활성화한 뒤 `⌘F`로 네이티브 입력을 선택했다. `needle`은 `1/8`과 본문의 첫 강조를 표시했고 Enter·`⌘G`로 2~8 뒤 1로 순환했다. `⇧Enter`는 1에서 8, `⌘⇧G`는 7로 이동했다. 화면에서 7번은 iframe 안의 첫 일치였고 네이티브 이전·다음 버튼도 동작했다. 창 활성화 전 최초 `⌘F`에 AX 변화가 없었던 시도는 성공으로 세지 않았다.
- iframe에서 한국어 검색어로 바꾸자 `5/5`를 표시했다. 총 결과 수는 5개였으며 CEF의 현재 선택 위치가 시작점에 영향을 주므로 새 검색을 항상 문서 1번에서 시작한다고 설명하지 않는다. 한국어는 붙여넣기로 입력했다.
- 불일치 검색은 결과 없음과 비활성 화살표, 빈 검색어는 결과 수 지움을 확인했다. 짧은 검색어와 빠른 다음·이전 입력 후 `needle`로 교체해 `7/8`을 표시했다. Esc로 닫고 `⌘G`로 같은 문서의 검색어를 다시 열었다. 이 실행은 모든 가능한 콜백 순서를 재현한 검증은 아니다.
- 탭 전환, 통합 검색·북마크 화면, 실제 본문 링크 탐색과 CLI 탭 닫기에서 찾기와 기억된 검색어를 정리했다. 다른 네이티브 화면에서 `⌘F`가 입력을 가져가지 않는 것도 확인했다.
- AI 작업 A의 주소 초안 `DraftKeep`을 입력한 뒤 `⌘F`를 열면 A만 paused·revision 2가 됐고 B는 active·revision 1을 유지했다. 주소 초안도 보존했고 A의 이전 revision 요청은 거절했다. 이후 다시 멈춘 A에서 본문 클릭·붙여넣기는 focus 1회·input 1회·`isTrusted:true`였으며 추가 revision 변경은 없었다.
- 이미지 로딩을 기다리는 B의 scoped load wait를 등록해 `pendingWaits:1`을 확인했다. 실제 `⌘F`와 검색은 B를 paused·revision 2로 바꾸고 해당 wait를 `TASK_CONTROL_LOST`, `outcomeUnknown:false`로 끝냈다. 대기는 0으로 돌아왔고 A는 paused·revision 4를 유지했다. 이미지 gate를 명시적으로 해제한 뒤 검증 작업 두 개를 종료했다.

## v1 결함과 v2 수정 경계

v1에서 A를 revision 3으로 재개했을 때 `needle 1/8` 막대가 남았다. 성공한 task resume이 기존 찾기 세션과 UI의 탐색 세대를 무효화하지 않은 경로를 확인했다. `src/browser.rs`의 `Command::TaskResume` 성공 분기에서 재개한 작업의 탭만 `expire_tab_refs`에 전달하도록 수정했다. 이 경로는 찾기 무효화와 해당 탭의 세대 변경을 수행하며, 실패한 resume이나 다른 작업의 탭에는 적용하지 않는다.

`task_resume_invalidates_only_its_find_session_after_success` 회귀 검사 한 개를 추가했다. `artifacts/find-page-v2-rust-tests.log`의 전체 137개 검사가 통과했고, 포맷·변경 형식 검사와 release 빌드 결과는 `artifacts/find-page-v2-validation.json`에 기록했다. 실제 수정 동작은 아래 동일 v2 바이너리에서 별도로 확인했다.

## v2 재개 회귀의 실제 확인

v2 SHA-256은 `458ee5ad7a075d2a0b3c35b1d75f037847976e7711720b7319529eea1e0ea427`이며 근거는 `artifacts/find-page-v2-native.json`이다. 최초 탐색은 키체인 승인 전 시간 초과였지만 사용자가 항상 허용한 뒤 재빌드 없이 같은 바이너리·탭의 HTTP 로딩 완료를 확인했다. 이번 승인 대기는 해소됐으며 재서명 후 무승인 실행을 해결한 결과는 아니다.

- 기본 `needle` 검색은 `1/8`, Enter는 `2/8`, `⇧Enter`는 `1/8`을 표시했다.
- AI 작업 A에서 `⌘F`는 A를 paused·revision 2로 바꿨다. 이전 revision 1의 resume은 `TASK_STALE_REVISION`으로 거절됐고 A의 검색어·포커스를 유지했다. B의 handoff·revision 2와 resume·revision 3도 A의 검색어·포커스를 바꾸지 않았다.
- A의 정상 resume은 active·revision 3을 반환했다. AX에서 찾기 막대가 사라지고 실제 스크린샷에서 주황·노랑 강조가 없어졌으며 본문 높이가 복원됐다. `⌘G`는 이전 검색어를 되살리지 않았고 A와 B 모두 active·revision 3을 유지했다.
- 다시 `⌘F`를 누르면 빈 입력으로 시작하고 A만 paused·revision 4가 됐다. 한국어 붙여넣기는 `1/5`를 표시했고 Esc로 닫혔다. 실제 IME 조합 검증은 아니다.
- 검증 작업 A는 finished·revision 5, B는 finished·revision 4였으며 각각 `tabs`와 `closingTabs`가 비었다. 원래 사용자 빈 탭 1만 남겼다.

이 결과는 v1의 AI 재개 후 찾기 잔존 결함에 대한 로컬 네이티브 회귀다. 모든 가능한 늦은 콜백 순서를 실제 앱에서 재현한 결과는 아니다.

같은 v2에서 작업 6개·공간 10개·단일 창 6개, 총 22개 로컬 CEF 회귀를 통과했다. 세 묶음은 모두 exit 0·빈 stderr이며 `artifacts/find-page-v2-suite.json`에 기록했다. `artifacts/find-page-v2-suite-state.json`의 `cleanupVerified:true`, 생성한 작업 7개의 finished·빈 탭 목록, 기존 사용자 빈 탭 1과 대기·캡처·생성 예약 0을 확인했다. 최초 샌드박스 시도의 읽기 명령은 소켓 연결 권한 오류 `SOCKET_IO_ERROR`, `outcomeUnknown:false`였고 실행 권한을 받은 재시도에서 통과했다. 실패 기록은 `artifacts/find-page-v2-suite-sandbox.json`과 `artifacts/find-page-v2-suite-state-sandbox.json`에 보존했다.

## v2 정상 재시작과 메모리 표본

네이티브 종료 메뉴를 실행한 뒤 앱 실행 세션의 exit 0으로 정상 종료를 확인했다. 같은 프로필과 바이너리로 재시작한 후 추가 승인 없이 로컬 HTTP 탭의 complete와 네이티브 화면을 확인했다. 검증 탭을 닫고 기존 사용자 빈 탭 1·대기·캡처·생성 예약 0으로 돌아왔으며, 빈 북마크 목록을 열어 확인한 뒤 닫았다. 이전 앱 PID 7119는 없고 검증용 HTTP 서버도 SIGTERM 후 exit 0으로 종료했다. 종료·재시작과 정리는 `artifacts/find-page-v2-validation.json`에 기록한다.

검사 정리 후 6개 프로세스의 RSS 합계는 세 표본 모두 721.109MiB였다. 같은 바이너리의 재시작·HTTP 탭 열기와 닫기·북마크 목록 확인 후에는 6개 프로세스에서 694.438, 694.438, 694.625MiB를 기록했다. 근거는 `artifacts/find-page-v2-after-tests-memory.json`과 `artifacts/find-page-v2-restart-memory.json`이다. RSS 합계는 공유 매핑을 중복 계산하며 고유 RAM 사용량이 아니다. 실행 이력과 대기 조건이 달라 이 차이나 이전 v3 측정과의 차이를 기능 변경의 절감 효과로 해석하지 않는다. 측정 조건은 [메모리 문서](memory.md)에 정리한다.

좁은 창, 실제 한글 IME 조합, 외부 사이트, 전체 네이티브 편집 단축키와 장시간 사용은 미검증이다. 본문에서 키 하나가 현재 입력 소스에 따라 한글 자모로 들어간 관찰을 IME 조합 성공으로 간주하지 않는다.

후속 `input-layout-v1`에서는 같은 v2 바이너리로 새 로컬 입력 fixture를 검사했다. `한글검증표식` 붙여넣기는 `1/1`을 표시했다. `가` 205자를 붙여넣으면 실제 AX 값은 200자가 되고 결과 없음·비활성 이전/다음 버튼이 표시됐다. 사이드바를 접어도 이 값과 포커스가 유지됐고 Esc로 막대를 닫았다. 본문의 CUA `r`·`k` 입력은 `ㄱㅏ` 및 trusted `insertText` 두 번으로 관찰됐지만 조합 이벤트는 없었다. 이 결과는 붙여넣기·일반 입력과 상한의 증거이며 실제 IME 조합의 증거가 아니다. 최소 창 크기 조절도 이번 도구 호출로 재현하지 못했다. 기록은 `artifacts/input-layout-v1-native.json`, 남은 수동 검사는 [입력 검증 절차](input-validation.md)를 따른다.

## 고정 엔진의 검색 계약

설치된 CEF 156의 `Find`에는 호출자가 지정하는 요청 ID 인자가 없다. 고정된 CEF `8db2f36735f06aacaf1bab832cb08da9d9445c55`와 Chromium `156.0.8078.17`의 공식 구현을 확인했다. 짧은 검색어를 지연 처리하는 동안 후속 찾기는 ID를 재사용할 수 있으므로 ID가 항상 1씩 증가한다고 계산하지 않는다.

CEF는 Chromium의 `FindTabHelper`를 거쳐 중지되거나 이전 검색 세션에 속한 결과를 거른다. UI의 `find_next=false`는 논리적인 새 검색을 뜻한다. backend는 새 검색에서 `StopFinding`으로 기존 세션과 강조를 지운 뒤, 실제 CEF `Find`의 `findNext`를 참으로 보내 현재 위치를 기준으로 일치 항목 선택을 요청한다. 고정 버전은 이 인자를 Chromium의 `find_match`로 전달한다. 같은 문서를 닫았다 다시 같은 검색어로 찾아도 새 세션으로 시작하며, 새 세션이라는 이유로 문서의 1번 결과를 보장하지 않는다.

같은 검색어의 다음·이전 요청이 진행 중이면 가장 최근 요청 하나만 보관해 완료 뒤 실행한다. 중지 token은 최신 요청과 정확히 일치해야 하며, UI는 거절된 새 요청과 이전에 승인된 요청의 경계를 모두 정리한다. 알 수 없는 결과 수나 위치의 `-1`을 0개 결과로 바꾸지 않는다. 양수 결과 수와 위치 0도 결과 없음으로 표시하지 않는다. 이 소스 계약과 위 버전별 로컬 화면 관찰을 구분한다.

고정 버전 근거:

- [CEF 검색 결과 필터 호출](https://github.com/chromiumembedded/cef/blob/8db2f36735f06aacaf1bab832cb08da9d9445c55/libcef/browser/browser_host_base.cc#L1470)
- [CEF 콜백 전달](https://github.com/chromiumembedded/cef/blob/8db2f36735f06aacaf1bab832cb08da9d9445c55/libcef/browser/browser_contents_delegate.cc#L667)
- [Chromium 검색 세션, 취소와 부분 결과](https://chromium.googlesource.com/chromium/src/+/refs/tags/156.0.8078.17/components/find_in_page/find_tab_helper.cc)
- [Chromium 검색 ID와 지연 큐](https://chromium.googlesource.com/chromium/src/+/refs/tags/156.0.8078.17/content/browser/find_request_manager.cc)

실행 결과와 남은 제약은 [작업 기록](work-log.md)에 추가한다.
