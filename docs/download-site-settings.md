# 현재 사이트의 다운로드 허용 설정 초기화

기준일: 2026-10-15. 기존 연속 다운로드 승인 기능에 현재 사이트 설정 조회와 저장된 허용 초기화를 추가하는 소스 변경이다. 실제 사용자 프로필과 앱을 실행하지 않았으며 네이티브 검증은 남는다.

## 범위와 동작

다운로드 목록의 맨 위에 현재 사이트 카드 하나를 표시한다. 주소 입력 초안이 아니라 선택된 main frame의 실제 HTTP(S) 주소에서 origin을 구한다. 현재 적용값이 허용일 때만 `허용 설정 초기화` 버튼을 활성화한다. 허용된 모든 사이트의 목록이나 새 ALLOW, BLOCK 설정 기능은 추가하지 않는다.

버튼은 해당 origin의 저장된 예외를 기본값으로 되돌린다. 공간들이 같은 프로필을 공유하므로 모든 공간의 같은 사이트 설정에 적용된다. 같은 origin의 대기 중인 Naru 권한 요청은 IGNORE로 취소한다. Chromium은 같은 페이지에서 함께 기다리던 다른 origin 요청도 취소할 수 있다. 이미 시작한 파일과 저장 완료 파일, 다른 종류의 권한, 쿠키, 로그인과 북마크는 이 기능의 변경 대상이 아니다. AI 작업의 제어권, revision이나 선택도 바꾸지 않는다.

설정 초기화가 모든 열린 문서의 내부 허용 상태까지 즉시 철회한다는 뜻은 아니다. 카드와 성공 안내에 **열린 페이지는 다시 열어 확인**하도록 표시한다. 자동 새로 고침이나 모든 파일 취소는 수행하지 않는다. 실제 여러 페이지와 iframe에서 즉시 철회하는 기능은 아직 완료 범위가 아니다.

## 설치 버전의 근거

설치 CEF `8db2f36735f06aacaf1bab832cb08da9d9445c55`와 Chromium `156.0.8078.17`의 고정 소스를 읽었다. 설치 헤더는 `.cache/cef/156.0.1/cef_macos_aarch64/include/cef_version.h`다. 원문 URL과 확인 당시 SHA-256은 `artifacts/download-site-v1-source-evidence.json`에 남겼다.

CEF의 GetContentSetting은 적용값을 반환한다. 두 URL이 비면 전체 사이트 기본값의 조회나 변경이 되므로 변경에는 비어 있지 않은 검증된 origin을 사용한다. `SetContentSetting(origin, 빈 secondary, AUTOMATIC_DOWNLOADS, DEFAULT)`로 초기화하며 UI thread의 초기화된 context는 즉시 실행 경로를 사용한다. DEFAULT는 조회 실패 때도 반환하므로 이를 성공으로 세지 않는다. [고정 CEF 구현](https://github.com/chromiumembedded/cef/blob/8db2f36735f06aacaf1bab832cb08da9d9445c55/libcef/browser/request_context_impl.cc#L572)

AUTOMATIC_DOWNLOADS의 등록 기본값은 ASK이며 requesting origin만 scope로 사용한다. 정확한 scheme, host, 유효 port를 유지하고 path와 query는 scope에 포함하지 않는다. DEFAULT는 해당 예외 삭제로 이어지지만 상위 정책이나 다른 패턴, 전역 기본값의 허용이 남을 수 있다. [설정 등록](https://github.com/chromium/chromium/blob/156.0.8078.17/components/content_settings/core/browser/content_settings_registry.cc#L197), [scope 처리](https://github.com/chromium/chromium/blob/156.0.8078.17/components/content_settings/core/browser/host_content_settings_map.cc#L172), [예외 삭제](https://github.com/chromium/chromium/blob/156.0.8078.17/components/content_settings/core/browser/content_settings_pref.cc#L193)

다운로드 제한기의 설정 observer는 최근 origin만 확인하고 대기 콜백을 완료하지 않는다. 다른 origin을 요청한 기존 문서에서는 이전 origin의 허용 상태가 남을 수 있다. IGNORE는 CancelOnce로 연결되며 그 WebContents의 다른 대기 다운로드도 취소할 수 있다. 이는 저장된 설정, 이미 존재하는 문서 상태, 대기 요청을 구분해야 하는 근거다. [다운로드 제한기](https://github.com/chromium/chromium/blob/156.0.8078.17/chrome/browser/download/download_request_limiter.cc#L342), [권한 결정](https://github.com/chromium/chromium/blob/156.0.8078.17/chrome/browser/download/download_permission_request.cc#L42)

## 호출 흐름과 보호

`control_panel`은 다운로드 패널이 열렸을 때만 현재 사이트를 조회한다. 조회 중 CEF가 재진입할 수 있어 SHELL borrow 밖에서 호출하고 fresh browser snapshot으로 표시 대상을 다시 확인한다. 고정 카드의 기존 컨트롤을 재사용하며 조회 재진입만으로 버튼이나 포커스를 제거하지 않는다. 페이지가 바뀌거나 버튼이 비활성화됐을 때 그 버튼이 실제 포커스를 가진 경우에만 닫기로 이동한다.

`browser::capture_download_site_target`은 기존 CurrentPageTarget에 문서 세대와 소유자, revision, 상태를 추가한다. `read_download_site_page`는 기존 주소 검사를 재사용해 실제 main frame 주소 조회 전후 현재 공간, 선택, 문서와 탐색 세대, 소유권을 검사한다. HTTP(S) 문서가 준비되지 않았거나 실패, 종료, 다른 overlay 상태이면 변경 대상이 아니다. 다운로드 패널은 CEF 화면을 숨기므로 기존 페이지 표시 전용 가드를 그대로 쓰지 않고 별도 네이티브 가드를 적용한다.

`download_permission_runtime::site_snapshot`은 BrowserHost의 실제 RequestContext에서 설정을 읽는다. `reset_site`는 표시 당시 대상과 실제 origin, 허용 상태, 네이티브 창과 입력기 상태를 검사한다. 같은 origin의 pending callback을 큐에서 먼저 꺼내 borrow 밖에서 IGNORE하고, 실제 주소와 제어권을 다시 확인한다. 그 사이 문서가 바뀌면 setter를 실행하지 않으며 취소한 대기는 오류 안내에서 구분한다.

setter에는 항상 AUTOMATIC_DOWNLOADS와 DEFAULT만 전달한다. 반환값이 없으므로 같은 context와 origin을 재조회해 ASK 또는 BLOCK일 때만 저장된 허용이 남지 않음을 확인한다. ALLOW나 알 수 없는 값이면 초기화 시도와 확인 실패를 구분해 표시한다. setter 실행 후 화면이 바뀌어도 이미 요청한 설정 변경을 취소했다고 표현하거나 이전 설정으로 되돌리지 않는다.

## 자원과 검증

새 파일 저장, 승인 목록 캐시, worker, 타이머와 웹 UI renderer는 없다. 네이티브 카드 하나, 라벨 7개, 버튼 하나와 현재 대상 하나를 유지한다. 현재 대상에는 기존 최대 8,192바이트 URL과 최대 2,048바이트 origin 등이 포함된다. 현재 탭 수와 무관하게 승인 목록을 누적하지 않는다. 기존 권한 대기 상한 4개와 파일 다운로드 상한은 그대로 유지한다. 실제 RAM 증감은 측정하지 않았다.

새 검사 8개를 포함한 Rust 단위 239개, 기본 기능과 CLI 구성 전체 타깃 Cargo 검사, format과 diff 검사가 통과했다. 최초 fake CEF 검사의 엔진 문자열 호출 오류와 수정, 생성물 정리는 작업 기록과 `artifacts/download-site-v1-validation.json`에 기록했다. fake CEF RequestContext 검사는 전달한 origin, content type, DEFAULT와 readback 처리의 증거이며 Chromium 설정 파일 반영의 실행 증거는 아니다.

후속 실제 앱 검증은 허용 상태 조회, 초기화 후 재탐색과 재시작, 같은 사이트의 여러 공간과 iframe, 대기와 진행 파일 구분, 설정 중 탐색과 종료, 키보드 포커스, IME와 최소 창 배치다. 키체인 반복 요청 중단은 유지한다. 전체 사이트 권한 관리와 독립 브라우저 전체 완성으로 확대하지 않는다.
