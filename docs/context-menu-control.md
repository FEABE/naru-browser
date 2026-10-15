# 페이지 우클릭 메뉴와 AI 제어권

Naru의 사용자 공간과 AI 공간은 같은 창과 프로필을 사용한다. 사용자가 본문을 우클릭하면 기존 네이티브 입력 경로가 해당 AI 작업을 중지한다. 메뉴를 연 뒤 작업이 재개되거나 페이지가 바뀌는 경우에는 메뉴 명령도 실행 직전에 다시 확인해야 한다.

## 확인한 엔진 경로

고정 CEF 커밋 `8db2f36735f06aacaf1bab832cb08da9d9445c55`, Chromium `156.0.8078.17`을 확인했다. 설치된 CEF 헤더와 `artifacts/context-menu-source`의 원문은 소스 계약의 근거이며 실제 네이티브 재현 결과가 아니다.

- macOS 기본 메뉴는 열린 동안 앱 작업을 처리할 수 있다. 메뉴 항목은 `itemSelected:`를 사용하므로 Naru의 일반 편집 메뉴 selector 검사만으로 명령을 다시 검증하지 못한다. [CEF 메뉴 실행](https://github.com/chromiumembedded/cef/blob/8db2f36735f06aacaf1bab832cb08da9d9445c55/libcef/browser/native/menu_runner_mac.mm#L29), [Cocoa 항목 실행](https://github.com/chromium/chromium/blob/156.0.8078.17/ui/menus/cocoa/menu_controller.mm#L466).
- `OnContextMenuCommand`가 1을 반환하면 기본 명령을 실행하지 않고, 0이면 CEF가 실행한다. Naru는 기본 메뉴 표시와 명령을 유지하며 유효성 검사만 추가한다. [CEF 명령 분기](https://github.com/chromiumembedded/cef/blob/8db2f36735f06aacaf1bab832cb08da9d9445c55/libcef/browser/menu_manager.cc#L240).
- Cocoa의 메뉴 닫힘은 즉시 `OnContextMenuDismissed`를 호출하지 않는다. CEF는 `ActivatedAt` 뒤 종료를 알리려고 `OnMenuClosed`를 게시한다. 명령 검사에서 대상을 소비하고 종료 알림에서 해제하는 수명이 이 계약에 맞는다. [지연 종료](https://github.com/chromiumembedded/cef/blob/8db2f36735f06aacaf1bab832cb08da9d9445c55/libcef/browser/menu_model_impl.cc#L943).
- 편집 명령은 실행 시 `GetFocusedFrame()`을 다시 조회한다. Chromium의 Cut, Paste도 현재 포커스된 입력 대상으로 전달한다. 원래 메뉴 프레임을 CEF가 자동으로 고정한다고 가정하지 않는다. [CEF 편집 명령](https://github.com/chromiumembedded/cef/blob/8db2f36735f06aacaf1bab832cb08da9d9445c55/libcef/browser/menu_manager.cc#L473), [Chromium Paste](https://github.com/chromium/chromium/blob/156.0.8078.17/content/browser/web_contents/web_contents_impl.cc#L7151).

일반 페이지의 기본 항목은 뒤로, 앞으로, 인쇄, 소스 보기다. 편집 가능한 내용과 선택 상태에 따라 편집 항목이 달라진다. 제어권 보호 v2는 기본 메뉴만 검사했고, 후속 소스는 아래 링크 주소 열기 항목을 추가한다.

## 대상과 수명

`BrowserClient`의 `ContextMenuHandler`는 getter마다 새 객체일 수 있으므로 대상은 `Tab`에 보관한다. 탭, 공간, 메인 탐색과 문서 세대, 소유 작업의 ID와 revision 및 상태, 원본 프레임 ID를 값으로 보관한다. 링크 항목에는 아래 제한을 통과한 URL 하나만 추가한다. 메뉴 세대 u64를 함께 사용해 검사 중 새 메뉴가 열렸다 닫혀도 이전 검사가 통과하지 못하게 한다. CEF Browser, Frame, params와 model 참조는 추가 보관하지 않는다.

메뉴를 열 때 현재 탭과 공간, renderer 상태, 실제 창과 뷰의 표시 상태를 확인한다. 명령은 대상을 한 번 꺼내 검사한다. 네이티브 UI와 CEF 프레임 getter를 호출할 때 STATE와 SHELL borrow를 유지하지 않으며, 반환 뒤 같은 대상과 메뉴 세대인지 다시 확인한다. 창이 비활성이거나 전체 패널, 공간 보기와 시작 화면이 표시되면 실행하지 않는다.

같은 대상의 AI 작업이 active라면 기존 `handoff_page_input`으로 해당 작업만 인계한다. 그 인계가 만든 탐색 세대 1 증가, revision 1 증가와 paused 전환만 허용한다. 문서 세대, 공간, 선택, 프레임과 메뉴 세대가 함께 바뀌면 거절한다. 메뉴를 연 뒤 재개된 새 revision은 자동으로 다시 중지시키지 않는다. 사용자 탭과 이미 paused인 작업은 추가 인계 없이 기본 명령으로 넘긴다.

프레임 ID는 캡처 시와 최종 허용 직전에 확인한다. 같은 프레임의 문서 교체는 ID만으로 구분할 수 없으므로 모든 프레임의 browse와 load-start에서 메뉴를 무효화한다. 다른 iframe의 탐색도 메뉴를 취소할 수 있는 보수적인 정책이다. 이미 전달된 편집 명령을 되돌리거나 문서 내부 DOM 변경 전체를 검출하지는 않는다.

만료된 명령은 기본 실행을 차단한다. 같은 페이지가 아직 표시돼 있으면 기존 상태 표시줄에 메뉴를 다시 열도록 안내한다. 새 창과 포커스 이동을 만들지 않는다. 명령 소비, 메뉴 종료, 다음 메뉴와 탭 닫힘에서 대상을 해제하며 새 worker, 반복 타이머나 영속 저장소는 없다.

## 링크 주소를 새 탭에서 열기

CEF 기본 메뉴에는 링크를 새 탭에서 여는 항목이 없다. 후속 소스는 유효한 HTTP(S) 링크에 `링크 주소를 새 탭에서 열기`를 추가한다. `ContextMenuParams::GetLinkUrl`의 필터된 주소를 기존 `address::normalize_url`로 정규화한다. 최대 8,192바이트를 적용하고 자격증명, 제어문자와 지원하지 않는 주소를 거절한다. 검색어 변환은 하지 않는다. `GetUnfilteredLinkUrl`은 CEF 계약상 주소 복사용이므로 탐색에 쓰지 않는다. [CEF URL 계약](https://github.com/chromiumembedded/cef/blob/8db2f36735f06aacaf1bab832cb08da9d9445c55/include/cef_context_menu_handler.h#L228).

메뉴를 열 때 주소를 고정하고 명령 직전에 위의 페이지와 프레임, 제어권 검사를 거친다. 필요한 인계가 끝난 뒤의 소유권과 원래 공간을 기존 링크 생성 큐에 전달한다. 같은 공간에 AI가 소유하지 않은 배경 탭을 만들며 현재 페이지와 선택을 유지한다. 활성 작업이면 해당 작업만 중지하고 다른 작업은 유지한다. 새 탭도 Chromium의 페이지 자원을 사용하며, 사용자 요청에는 기존 자동화 탭 예산을 적용하지 않는다.

명령을 수락한 뒤에는 고정 URL을 여는 요청이다. 정상 메뉴 종료와 단순 선택 변경 때문에 취소하지 않으며 이후 문서 이동에 맞춰 다른 URL로 바꾸지 않는다. 원본 탭이 닫히거나 공간과 소유권, revision이 바뀌면 큐 또는 생성 완료 검사에서 거절한다. 메뉴의 임시 generation은 정상 종료 때도 바뀌므로 생성 큐의 유효성으로 쓰지 않는다. 실패 뒤 이미 끝낸 인계를 자동 재개하지 않고 기본 메뉴 동작으로도 넘기지 않는다. 현재 원본 페이지가 표시돼 있으면 기존 상태 표시줄에 실패를 알린다.

기존 `OpenLinkTask` 경로를 사용하므로 URL을 독립 탐색한다. CEF 초기 탐색에는 빈 referrer와 연결되지 않은 opener가 전달된다. DOM click, JavaScript, `<a download>`, 링크의 referrer와 rel 정책을 재현하지 않으며 page URL을 임의 Referer로 추가하지 않는다. 이 한계는 Naru의 URL 기반 생성 경로에 해당하며 CEF가 원래 팝업을 만드는 경로까지 일반화하지 않는다. [고정 Alloy 생성 코드](https://github.com/chromiumembedded/cef/blob/8db2f36735f06aacaf1bab832cb08da9d9445c55/libcef/browser/alloy/alloy_browser_host_impl.cc#L132).

## 검증 경계

제어권 보호 v2 시점에는 Rust 단위 268개와 기본 구성, CLI 전용 구성의 전체 타깃 컴파일 검사 및 format이 통과했다. 이 보호 경로의 신규 검사는 9개다. 실제 Rust 상태와 비활성 CEF handle로 대상 수명을 확인하며, 하위 프레임 검사는 실제 browse와 load-start handler를 호출해 메인 문서 세대가 그대로인데도 메뉴가 거절되는 것을 확인한다. 프레임 ID와 재진입 검사는 주입한 조회 함수로 정책을 확인한다. 실제 CEF 프레임 getter와 AppKit 메뉴, Chromium 편집 결과의 실행 증거는 아니다. `artifacts/context-menu-v2-check.json`과 `context-menu-v2-rust-tests.log`, 작업 기록에 소스 해시와 중간 파일 정리를 남겼다.

링크 추가 후 최종 context-link-v2는 Rust 단위 273개, 두 구성의 전체 타깃 컴파일 검사와 format을 통과했다. 신규 5개는 주소 정규화와 상한, getter와 메뉴 변경의 재진입, 인계 후 소유권, 정상 메뉴 종료 후 요청 유지, 원본 공간·소유권·닫힘과 큐 실패를 검사한다. 최초 실행은 다른 공간의 탭을 공간 전환 없이 선택한 테스트 설정 때문에 1개가 실패했다. 공간 전환과 선택의 성공을 명시적으로 확인하도록 테스트만 수정한 뒤 전부 통과했다. 실제 CEF 메뉴 객체와 생성 콜백 연결은 소스 검토 근거이며 엔진 실행 증거는 아니다. `artifacts/context-link-v1-check.json`, `context-link-v2-check.json`과 각각의 검사 로그에 실패와 재검사 결과를 구분했다.

후속 실제 앱 검사에는 정상 Copy와 Paste, 메뉴 취소, 메뉴가 열린 동안 task resume, 공간 전환과 닫기, iframe 탐색과 포커스 변경, 실제 메뉴의 key-window 상태가 필요하다. 메뉴가 재개나 iframe 탐색 뒤 계속 열려 실제 잘못된 문서에 입력되는 현상을 재현한 것은 아니다. 최신 앱의 키체인 승인을 반복 요청하지 않으며 이 소스의 네이티브 검증을 이전 바이너리 결과로 대체하지 않는다.
