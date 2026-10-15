# 네이티브 새 탭 화면

이 문서는 네이티브 새 탭 화면의 표시, 입력, 포커스와 AI 제어권 계약을 설명한다. start-page-v2의 제출 흐름, ui-focus-v3의 편집 중 오버레이 전환 보완, 후속 native-focus의 키보드 진단을 버전별로 구분한다. 소스 변경과 실행 결과는 따로 기록하며 빌드, 네이티브 UI, 로컬 CEF 검증과 생성물 정리는 [작업 기록](work-log.md)을 기준으로 한다. native-focus-v3에서 진단을 끈 키보드 이동과 HTTP, 로컬 CEF 32개를 확인했다. 후속 surface-probe-v1은 별도 진단 빌드이며 이 검사 수를 이어받지 않는다. 사이드바 잘림은 초기 재현과 후속 정상 관찰이 함께 남아 있어 원인과 재현 조건이 미확정이다.

## 화면과 표시 조건

기존 단일 브라우저 창의 본문 영역에 AppKit `StartPage` 하나를 둔다. 제목, 주소·Google 검색 입력, 열기 버튼과 공유 북마크 진입 버튼으로 구성한다. 별도 창, HTML 홈, UI용 WebContents는 만들지 않는다. 상단 주소창과 세로 탭, 공간 표시·중지 버튼은 기존 경로를 유지한다.

주소 문자열만으로 새 탭을 판단하지 않는다. `Tab.native_start_page`는 앱 시작 시 생성한 빈 탭과 사용자가 명시적으로 만든 새 탭에만 부여한다. CLI 생성, 페이지 링크·팝업, DevTools, 기본 client, 북마크와 닫은 탭 다시 열기 경로에는 부여하지 않는다. 따라서 내용이 들어 있는 `about:blank` 팝업 위를 새 탭 화면으로 덮지 않는다.

표시하려면 이 생성 이력과 정확한 `about:blank` 주소, 탐색 완료·문서 준비 상태가 모두 맞아야 한다. 로딩·탐색 중지·오류·렌더러 복구 중이거나 거절된 탭에는 표시하지 않는다. 현재 선택된 탭과 공간이 일치해야 하며, 공간 둘러보기나 탭 검색·북마크·다운로드 화면을 열면 숨긴다. 선택된 탭이 없는 AI 공간에 새 탭을 자동 생성하지 않는다.

빈 주소가 아닌 명시적 탐색 또는 메인 프레임의 browse·address·commit 관찰은 표시 자격을 영구 해제한다. 이후 `about:blank`로 돌아와도 다시 부여하지 않는다. 기존 빈 탭의 reload는 자격을 유지할 수 있지만 탐색 epoch가 바뀌므로 이전 입력은 버린다.

## 입력과 AI 제어권

중앙 입력은 현재 `StartPageTarget { tab, space, navigation_epoch }` 하나에만 연결한다. 탭·공간·epoch가 바뀌거나 표시 대상이 사라지면 입력과 오류 안내를 초기화한다. 같은 대상을 유지한 배경 상태 갱신이나 오버레이 왕복은 입력을 보존한다. 탭별 초안 저장소나 재시작 복원은 없다.

주소·검색어 제출은 기존 `address::resolve`를 사용한다. 인식 가능한 주소는 정규화하고 일반 검색어는 Google 검색 URL로 변환한다. 빈 입력은 현재 대상이 여전히 유효한지 검증한 뒤 중앙 편집기로 포커스를 되돌리며 탐색이나 AI 인계를 하지 않는다. 지원하지 않는 scheme·자격증명 포함 주소·잘못된 형식·길이 초과는 기존 주소창 규칙대로 거절한다. 중앙 필드의 실제 입력값을 사용하며 상단 주소창의 값을 대신 읽지 않는다.

유효한 입력은 실행 직전에 현재 선택, 공간 소속, epoch와 준비 상태를 다시 확인한다. CEF의 실제 메인 프레임 주소도 `about:blank`인지 확인한다. 이 조회와 후속 동작은 상태 borrow 밖에서 수행한다. 오래된 화면이나 잘못된 입력은 AI 제어권을 바꾸지 않는다.

검증을 통과하면 기존 `ShellAction::Navigate`가 해당 탭의 현재 AI 소유 작업만 사용자에게 인계한 뒤 탐색한다. 화면 표시·중앙 입력 클릭·타이핑만으로 작업을 중지하지 않는다. 북마크 버튼은 같은 대상 검증 후 기존 공유 북마크 화면을 열며, 그 자체로 AI를 중지하지 않는다. 북마크의 실제 열기 동작은 기존 사용자 공간 정책을 따른다.

## 포커스와 오버레이

중앙 입력은 사용자가 클릭해 편집한다. 기존 새 탭 동작과 `⌘L`은 상단 주소창으로 간다. 새 탭 화면이 보일 때는 숨긴 CEF 문서로 포커스를 보내지 않는다. 첫 빈 문서의 로드 완료처럼 선택이 바뀌지 않는 갱신에서도 이 규칙을 적용하며, 이미 보이는 네이티브 입력·버튼의 포커스는 유지한다.

비어 있지 않은 입력을 제출하는 동기 호출 동안에는 `StartPage.submission`에 대상 하나를 임시 보관한다. AI 인계가 epoch를 바꾸며 중간 화면 갱신을 일으켜도 같은 탭·공간의 상단 주소창으로 자동 포커스가 이동하는 것을 억제한다. epoch 검증을 우회하거나 다른 탭의 포커스를 막는 용도가 아니다. 중복 제출은 받지 않으며 호출이 성공 또는 실패로 반환하면 이 임시 상태를 지운다. 성공 후에도 실제 선택이 같은 탭·공간이고 새 탭 화면과 오버레이가 숨겨져 있을 때만 CEF 본문에 포커스를 준다.

대상 변경 전과 화면을 숨길 때 중앙 입력의 공유 AppKit field editor가 현재 responder와 같은 포인터인지 확인해 정리한다. 화면 내부의 responder도 함께 정리하되 다른 입력창의 편집기는 건드리지 않는다. `makeFirstResponder`와 CEF 호출은 `SHELL`·`STATE` borrow 밖에서 수행한다. 한글 조합 중에는 중앙 입력의 제출 동작을 실행하지 않는다. 실제 IME 조합 검증은 별도 실행 결과가 필요하다.

오버레이를 닫으면 기존 선택 복귀 경로를 사용한다. 같은 대상의 중앙 입력값을 보존한다는 계약이며, 중앙 편집기의 커서·선택 범위까지 복원한다는 뜻은 아니다. 다른 탭으로 이동했다가 돌아오면 이전 초안은 복원하지 않는다.

후속 `ui-focus-v2` 실제 검사에서는 중앙 입력을 편집한 채 사이드바 북마크를 열면 초안이 사라졌다. 같은 탭·epoch에서 Tab으로 편집을 먼저 끝낸 대조군은 보존됐다. `ui-focus-v3` 소스는 `sync`에서 대상 변경뿐 아니라 곧 시작 화면을 숨길 경우에도 기존 편집기를 먼저 종료한다. 이 호출을 상태 borrow 밖, 입력의 `setEnabled(false)`보다 앞에 두며 같은 대상의 문자열을 직접 복사하거나 다시 쓰지 않는다. 새 대상의 초안 초기화와 AI 제어권 검증은 유지한다. 이전 빌드의 중앙 북마크 버튼 검사와 이번 사이드바 진입 검사를 구분한다.

창의 키 뷰 목록은 최초 배치 후와 실제 화면 표시 전환 뒤에 갱신한다. 매 입력이나 주기 갱신마다 재계산하지 않는다. `ui-focus-v2`에서 중앙 입력→주소창 Tab은 이동했고, 주소창→중앙 입력의 Tab, ShiftTab은 주소창에 머물렀다. 당시 숨긴 Chromium 자식의 키 뷰 참여는 진단 가설이었으며, 이 추가 갱신만으로 전체 키보드 접근성을 해결했다고 판정하지 않았다.

최종 `ui-focus-v3` 네이티브 검사에서 편집 중 사이드바 북마크·통합 검색·다운로드 왕복의 초안 보존을 확인했다. 배경 AI 생성·HTTP 완료 뒤 사용자 선택·초안, A 작업만 인계되는 경계, 사용자 중앙 HTTP 탐색과 본문이 있는 빈 주소 팝업의 실제 입력도 확인했다. Rust 194개와 같은 바이너리의 로컬 CEF 회귀 32개가 통과했다. 주소창에서 시작 화면으로 향하는 Tab·ShiftTab 문제는 해당 빌드에도 남았다. 실제 최소 창·IME 조합·다크 모드·외부 검색·RAM 비교는 미검증이다.

### 숨긴 CEF로 향하는 Tab 보정

후속 `native-focus-v1`에서는 주소창의 `nextValidKeyView`와 중앙 입력의 `previousValidKeyView`가 숨긴 `RenderWidgetHostViewCocoa`를 반환했다. 해당 뷰는 `hidden`, `ancestor_hidden`, `canBecomeKeyView`가 모두 true였고, Tab과 ShiftTab 뒤 실제 first responder가 됐다. 같은 이벤트 안에 `OnGotFocus`는 있었지만 `OnSetFocus`는 없었다. 따라서 기존 CEF 포커스 요청 거절만으로 이 경로를 막지 못했다. 근거는 `artifacts/native-focus-v1-app.log`와 `artifacts/native-focus-v1-native.json`이다.

`native-focus-v2`의 `correct_hidden_cef_tab_focus`는 원래 `NSApplication.sendEvent` 처리를 마친 뒤 실행한다. Command, Control, Option이 없는 Tab 키 이벤트만 확인하며 원본 Shift 값으로 역방향을 구분한다. 이벤트의 창이 현재 key window와 같고 실제 first responder가 CEF 본문 하위의 숨긴 뷰일 때만 보정한다. AppKit의 다음 또는 이전 유효 후보를 따라가며 숨긴 후보를 건너뛴다. 같은 창의 보이는 `canBecomeKeyView` 후보에 도달하면 현재 창과 responder가 바뀌지 않았는지 다시 확인한 뒤 포커스를 한 번 이동한다. 순환, 다른 창, 후보 512개 상한에 도달하면 멈춘다.

입력 이벤트를 소비하거나 다시 보내지 않고, 수동 키 뷰 목록이나 텍스트 필드 전용 순서를 만들지 않는다. 보이는 버튼과 CEF 본문도 AppKit 후보 순서를 따른다. 상태 borrow 밖에서 뷰를 조회하고 포커스를 변경한다. 시작 화면의 CEF가 숨겨져 있으면 기존 `observe_page_input`의 가시성 검사에서 제외되므로 키보드 이동 자체가 AI 작업을 인계하지 않는 소스 경계도 유지한다. 이 설명은 OS의 FKA 활성 설정을 실제 검증했다는 뜻이 아니다.

v2는 Rust 194개, 전체 타깃 검사, format과 release 빌드를 통과했다. 네이티브 결과는 `artifacts/native-focus-v2-native.json`에 기록했다. 진단을 켠 실행은 주소창 Tab→중앙, 중앙 ShiftTab→주소창 두 방향을 확인했다. 같은 바이너리에서 진단을 끈 실행은 주소창과 중앙 입력 사이 Tab, ShiftTab 네 방향을 확인했다. 중간 빌드의 HTTP와 로컬 CEF 회귀는 실행하지 않았으며 ui-focus-v3의 32개 통과 결과를 이 빌드의 결과로 합산하지 않는다.

### 최종 native-focus-v3 검증

최종 v3는 Rust 194개, 전체 타깃 검사, format, release 빌드를 통과했다. 같은 바이너리의 로컬 CEF 회귀는 select 10개, tasks 6개, spaces 10개, single_window 6개로 총 32개다. 모두 exit 0, 빈 stderr였고 cleanup을 확인했다. 근거는 `artifacts/native-focus-v3-suite.json`과 `artifacts/native-focus-v3-suite-state.json`이다.

진단을 모두 끈 상태에서 주소창과 중앙 입력 사이 Tab, ShiftTab 네 방향을 최종 회귀 뒤 다시 확인했다. Tab으로 중앙에 진입해 실제 입력했고 북마크, 검색, 다운로드 왕복 뒤에도 초안을 유지했다. B의 배경 HTTP 생성과 완료 중 A의 선택과 편집 초안을 유지했고 재클릭 없이 입력을 이어갔다. A 공간에서 Tab과 입력은 A/B 모두 active revision 1을 유지했다. 유효한 HTTP 주소를 Enter로 제출할 때만 A paused revision 2, B active revision 1이 됐다. 근거는 `artifacts/native-focus-v3-native.json`, `artifacts/native-focus-v3-ai-keyboard.json`, `artifacts/native-focus-v3-handoff.json`이다.

보이는 웹 본문에서 입력란→Tab→버튼→Return으로 내용이 있는 `about:blank` 팝업을 만들었다. 팝업의 Tab과 붙여넣기 입력값은 CLI read와 일치했다(`artifacts/native-focus-v3-popup-read-after.json`). 최종 로컬 HTTP는 추가 키체인 질문 없이 완료됐으나 빌드 사이의 키체인 권한 유지까지 해결한 근거는 아니다.

네이티브 검증 작업 2개와 회귀 작업 9개, 총 11개 자체 작업을 종료했고 원래 사용자 탭 하나(ID 1)와 빈 중앙 입력을 보존했다. 대기 요청, 생성 예약, 캡처, 다운로드 대기는 0이다. 자체 서버는 exit 0으로 종료했고 최종 앱 PID 61776은 유지했다. 정리 근거는 `artifacts/native-focus-v3-native-cleanup.json`과 `artifacts/native-focus-v3-final-state.json`이며 생성물 정리 상세는 작업 기록을 따른다.

### 사이드바 표면과 진단의 영향

native-focus-v1에서 사이드바를 펼치면 검색과 새 탭 장식 배경의 왼쪽 모서리가 잘렸다. `setNeedsDisplay`만으로는 해결되지 않았고, 입력 없는 두 번째 화면 확인에서도 남았다. native-focus-v2에서는 좌표 진단을 켜면 정상으로 보였지만 같은 바이너리에서 진단을 끄자 잘림이 다시 발생했다. 진단에서 view와 model의 x좌표는 12였고 presentation 폭은 이전 값에서 새 값으로 수렴했다. 음수 좌표라는 근거는 없었으며, 조회에 따른 동기화와 시간 경과의 영향은 구분하지 못했다.

native-focus-v3 소스는 frame이 바뀐 검색과 새 탭 장식 배경만 골라 전체 배치와 `SHELL` borrow가 끝난 뒤 `displayIfNeeded`를 호출한다. 보이는 표면에만 적용하고 다른 버튼이나 CEF 뷰의 배치를 바꾸지 않는다. v3의 초기 진단 OFF 실행에서도 사이드바 잘림이 다시 발생했다. 이후 같은 실행의 접기·펼치기 두 차례와 같은 바이너리 재실행 후에는 정상이었고, 독립 CUA 캡처에서도 왼쪽 여백 24px와 둥근 모서리를 확인했다. 초기 실패 관찰은 유지하며 후속 정상 표시만으로 해결을 판정하지 않는다. 관련 스택 샘플에서 키체인 대기는 별도 worker에 있었고 main thread는 AppKit 이벤트 대기 상태였다. 키체인 대기와 잘림의 인과관계는 미확정이다. 근거는 `artifacts/surface-probe-v1-baseline.json`이다.

### 선택형 진단

`NARU_FOCUS_DIAGNOSTICS=1`은 첫 네이티브 Tab 이후 최대 128줄을 기록한다. `NARU_SURFACE_DIAGNOSTICS=1`은 첫 사이드바 전환 이후 최대 32표본을 기록하며 표본마다 검색과 새 탭 배경 두 행을 남긴다. 둘 다 기본값은 꺼짐이고 현재 프로세스의 임시 계수만 사용한다. 입력값, URL, 페이지 내용은 출력하지 않는다. 진단을 켠 화면만으로 문제 해결을 판정하지 않으며 기본 실행에서도 다시 확인한다.

후속 `surface-probe-v1`의 `NARU_SURFACE_PROBE`는 getter별 조회 또는 `noop` 대조군을 최대 16회 비교하기 위한 진단이다. 미지정 기본값은 OFF이며 기존 포커스나 표면 좌표 진단이 켜져 있으면 실행하지 않는다. 제품 표시 동작을 고치는 변경이 아니며 새 캐시, 타이머, 웹 UI를 추가하지 않는다. format과 release 빌드가 통과했고 앱 SHA-256은 `0a83c27b979736846c67070c98a877c95ad6817439eab182e8dc5e446d76362e`다. 진단만 추가한 이 빌드에는 추가 Rust 검사, 전체 CEF 32개 회귀와 모의 DOM 검사를 실행하지 않았다. native-focus-v3의 194개와 32개 결과를 합산하지 않는다.

기본 모드에서 접기·펼치기 한 주기 뒤 정상 표시와 진단 로그 0건을 확인했다. 같은 바이너리의 `noop` 모드에서는 토글 18회마다 접근성의 접힘·펼침 상태를 확인했고 로그는 정확히 sequence 1~16까지만 남았다. 마지막 스크린샷은 정상이었지만 18회 모두 이미지를 판독한 것은 아니다. 잘림이 나타나는 대조군을 확보하지 못해 getter별 실험은 실행하지 않았고 어느 조회가 표시를 바꾸는지도 판정하지 않았다.

마지막 기본 모드에서 Cmd+T 후 Tab으로 중앙 입력에 진입하고 북마크 왕복 뒤 초안을 보존했다. 중앙 HTTP Enter 뒤 로딩 완료와 CLI 읽기를 확인했으며 추가 키체인 질문은 없었다. 최초 기본 모드 종료는 teardown watchdog exit 2였고, 이후 `noop`과 기본 모드 HTTP 검증 후 종료는 모두 exit 0이었다. `noop`의 스택 표본에는 키체인 decrypt 대기가 없었으며 첫 종료 오류의 원인은 미확정이다. 최종 재실행은 진단 OFF, PID 68075로 유지하며 원래 사용자 탭 하나와 빈 입력만 남겼다. 작업과 대기 자원은 0이고 이 재실행 뒤 HTTP는 다시 검사하지 않았다. 자체 서버 PID 67440은 exit 0으로 종료했다. 근거는 `artifacts/surface-probe-v1-native.json`, `artifacts/surface-probe-v1-validation.json`, `artifacts/surface-probe-v1-http.json`, `artifacts/surface-probe-v1-final-state.json`과 작업 기록이다.

FKA 활성 설정, 실제 IME 조합, 최소 창, 다크 모드, 외부 서비스와 RAM 비교는 이 후속 검사에서 미검증이다. 중앙 입력 간 네 방향 이동 확인을 전체 키보드 접근성이나 브라우저 완성으로 확대하지 않는다.

## CLI와 캡처 경계

중앙 화면은 네이티브 브라우저 UI다. 기존 CLI의 DOM 읽기와 screenshot은 CEF 문서를 대상으로 하므로 이 입력 필드·버튼·문구를 읽거나 촬영하는 API가 아니다. 네이티브 화면 확인과 CLI 문서 상태 확인을 구분한다.

읽기, screenshot, 행동의 frame·isolated-world 준비 단계는 표시 자격을 유지한다. 권한 검사와 host 확보를 거쳐 페이지 변경 행동을 최종 `Step::Evaluate`로 전달하는 시점에만 자격을 해제하고 CEF 문서를 드러낸다. 성공 응답을 기다려 해제하는 구조가 아니므로, 해당 실행이 실패해도 자격을 되살리지 않는다. 새 범용 JavaScript 실행 명령을 추가한 것은 아니다.

## 비용과 검증 범위

창당 고정 AppKit 화면과 현재 대상 하나, 제출 중인 임시 대상 최대 하나, 탭당 표시 자격 bool을 추가한다. 제출 상태는 큐나 탭별 저장소로 늘어나지 않는다. 별도 DB·영속 초안·반복 타이머·백그라운드 검색 요청·이미지 자산·UI renderer는 없다. 기존 빈 탭의 CEF 인스턴스는 유지하므로 탭 자체의 Chromium 비용을 제거한 것은 아니다. 전후 RAM 절감은 측정하지 않았다.

순수 Rust 검사는 생성 이력·문서 상태에 따른 표시 조건, 현재 탭·공간·epoch 검증, 잘못된 입력의 제어권 보존과 최소 본문 영역의 frame 배치를 다룬다. 이는 실제 화면·키보드 동작의 증거와 구분한다. 네이티브 후속 검사는 중앙 입력의 정상·오류 제출, 동일 대상 오버레이 왕복, 배경 탭 작업 중 이어쓰기, AI 작업별 인계, 한글 조합, 좁은 창과 내용이 있는 빈 주소 팝업을 확인해야 한다. 외부 Google 검색과 전체 제품 완성은 이 정적 검사 통과로 주장하지 않는다.

v1은 Rust 194개·전체 타깃·format 검사와 release 빌드를 통과했다. 실제 네이티브 검사에서 중앙 입력의 HTTP 탐색, 본문이 있는 빈 주소 팝업, 동일 대상의 북마크 왕복과 배경 AI 로딩 중 초안 보존, A 작업만 paused revision 2로 바뀌고 B는 active revision 1을 유지하는 인계를 확인했다. 다만 빈 중앙 입력에서 Return 후 상단 주소창으로 포커스가 옮겨졌고, AI 인계 중 생성된 상단 편집기가 HTTP 완료 후에도 빈 주소 표시를 유지하는 두 문제를 발견했다. 이 결과는 `artifacts/start-page-v1-native.json`에 남아 있으며 v1의 전체 CEF 회귀 검사는 실행하지 않았다. 위 v2 계약은 이 두 문제를 보완한 소스 설명이고, 최종 v2 검사·빌드·실제 재검증 결과는 작업 기록에서 별도로 확인한다.

## 소스 근거

- [control_panel.rs](../src/control_panel.rs): `StartPage`, `start_page_target`, `start_page_frames`, `clear_start_page_responder`, `focus_start_page_if_visible`, `start_page_action`과 관련 순수 검사.
- [mac.rs](../src/mac.rs), [control_panel.rs](../src/control_panel.rs): 원래 이벤트 전달 뒤 `correct_hidden_cef_tab_focus`, 선택형 포커스 진단과 표면 좌표 진단.
- [browser.rs](../src/browser.rs): 생성 경로의 `native_start_page`, `start_page_ready`, `observe_start_page_url`, `validate_start_page_target`, `navigate_from_start_page`, 최종 행동 `dispatch`와 포커스 처리.
- [start_page.rs](../src/start_page.rs): 탭·공간·epoch로 구성한 일시적 대상.
- [address.rs](../src/address.rs): 기존 주소·검색어 해석 및 입력 제한.
- [start-page.html](../tests/fixtures/start-page.html), [preview_fixture.py](../tests/preview_fixture.py): 로컬 탐색 목적지와 본문이 있는 `about:blank` 팝업 fixture. fixture 작성 자체는 실제 CEF 통과 증거가 아니다.
