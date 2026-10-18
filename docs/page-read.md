# 여러 프레임을 공통 예산으로 읽기

2026-10-18 저장된 `read-page TAB` 소스의 계약이다. CLI·공통 예산·DOM 읽기와 native 수집 흐름을 읽기 전용으로 대조했다. 공통 예산·참조 묶음·UTF-16 경계 보완을 포함한 [v9 통합 빌드](../artifacts/ego-parity-v9-build.json)는 exit 0, 95.05초로 완료됐고 제품 소스·Cargo 36개의 전후 해시가 일치했다. 실제 앱·CEF·E2E와 전체 회귀 검사는 실행하지 않았다. 이전 프레임 포인터의 v8 결과와 이번 빌드 증거를 구분하며, 컴파일 성공으로 실제 여러 프레임 읽기·입력 성공을 대신하지 않는다.

## 명령과 기존 읽기의 관계

```sh
lightbrowser read-page TAB
lightbrowser task run ID REV read-page TAB
```

`TAB`은 양의 i32 탭 ID다. AI 소유 탭은 기존처럼 현재 작업 ID와 revision을 사용한다. wire 명령은 `{"type":"read_page","tab":TAB}`이며 사용자 공간이나 탭을 선택하는 명령이 아니다.

기존 `read TAB`은 메인 문서와 접근 가능한 같은 출처 프레임을 기존 공유 예산 안에서 순회한다. `read-frame TAB FRAME`은 지정한 CDP frame 한 문서를 읽는다. 두 형식은 유지하며 `read-page`는 현재 Browser에서 발견한 여러 문서를 한 요청과 공통 예산으로 읽는 별도 명령이다. [단일 프레임 읽기](frame-read.md)의 정확한 session/context 평가와 [입력 계약](frame-input.md)을 재사용한다.

## 수집 순서와 반환

먼저 현재 Browser의 root·iframe child session에서 새 inventory를 수집한다. 루트부터 너비 우선으로 문서를 읽으며 각 session tree에서 관찰한 형제 순서를 사용한다. 서로 다른 session의 프레임을 합친 순서가 전체 DOM 순서, 화면 위치나 읽기 순서와 같다는 보장은 없다. CEF Frame ID, URL, 이름으로 다른 frame을 대신 찾지 않는다.

각 frame은 현재 문서와 열린 Shadow DOM만 읽고 자식 문서는 재귀 순회하지 않는다. 다음 문서는 inventory에서 별도로 선택하므로 기존 same-origin 재귀 결과와 같은 자식 본문을 다시 합치지 않는다. 본문과 요소는 문서별로 한 번만 담고 전체 본문 사본을 추가로 만들지 않는다.

성공 응답은 다음 필드를 사용한다.

| 필드 | 의미 |
| --- | --- |
| `tab`, `epoch`, `documentEpoch` | 읽기에 결합한 탭과 메인 문서·탐색 세대 |
| `documents:[{frame,page}]` | 성공적으로 읽은 문서와 그 로컬 page 결과. frame에는 CDP 식별자와 session 등 inventory 메타데이터가 있음 |
| `unread:[{frame,reason}]` | 현재 요청에서 읽지 못했거나 예산 때문에 생략한 문서와 사유 |
| `frames` | 이번 inventory가 반환한 전체 프레임 메타데이터. 읽은 문서만의 목록이 아님 |
| `budget:{limit,used,remaining}` | 아래 여섯 공통 예산의 최초 한도·합산 사용량·남은 양 |
| `truncated` | inventory·본문의 잘림이나 읽기 예산 제한을 알리는 합산 표시. 개별 page의 상세 잘림 정보도 유지 |
| `atomic:false` | 여러 문서의 순차 관찰이며 원자적인 화면 snapshot이 아님 |
| `supportedActions`, `pointerActionsSupported` | `click`, `hover`, `fill`, `insert`, `press`, `check`, `select`와 `true`. 실제 대상 조건은 action 시 다시 검사 |

각 `page`의 URL·제목·본문·요소·viewport·scroll·focusedRef는 해당 frame 기준이다. focusedRef는 그 문서의 반환 ref 집합 안에 있거나 null이며 여러 문서의 값을 하나의 OS 포커스로 추정하지 않는다. 읽기에는 부모 표시·가림 검사가 없어 부모가 숨겨진 문서도 읽힐 수 있으며, 읽힌 요소를 최상위 창에서 보이는 것으로 판단하지 않는다. 부모 검사는 후속 action에 적용한다. password input value는 기존처럼 제외한다. 본문에 적힌 비밀값을 모두 식별하거나 신뢰할 수 있는 지시로 바꾸는 기능은 아니다.

## 공통 예산

문서마다 새 기본 예산을 부여하지 않고, 앞 문서에서 실제 사용한 양을 뺀 남은 예산을 다음 DOM 읽기에 전달한다. 모든 문서의 합계에 다음 한도를 적용한다.

| 예산 키 | 전체 한도 | 계산 범위 |
| --- | ---: | --- |
| `text` | 12,000 | 반환 body text의 UTF-16 code units. label·value·URL·title의 별도 제한을 포함한 문자열 총량이 아님 |
| `elements` | 120 | 반환 interactive 요소와 허용 ref 수 |
| `nodes` | 10,000 | walk 방문 횟수. label 등의 반복 텍스트 스캔도 포함 |
| `roots` | 64 | 각 문서의 walk가 들어간 열린 shadow root 수의 합계 |
| `options` | 240 | 반환 native select 옵션. 한 요소당 최대 40개도 유지 |
| `frames` | 16 | 각 page의 로컬 iframe 요소 메타데이터 합계. CDP inventory와 문서 읽기 횟수의 상한이 아님 |

별도로 루트를 포함해 문서 읽기는 최대 16번 시도한다. PAGE_LOADING인 문서도 시도 횟수에 포함한다. 문서 내부 순회 깊이는 각각 64이고, ancestor·style·focus 검사에는 기존 별도 상한을 적용한다. `nodes`는 모든 JavaScript 작업이나 실제 DOM 전체 크기를 계수한 값이 아니다.

DOM은 받은 `budgetLimit`과 실제 `budgetUsed`를 반환한다. Rust는 한도와 사용량 범위, UTF-16 본문 길이, 반환 요소·옵션·iframe 메타데이터 수를 대조한 뒤 차감한다. node 방문과 shadow root 사용량은 DOM이 보고한 수치를 범위 검사하며 결과 요소 수로 추정하지 않는다. 이전 문서가 사용한 예산을 다음 문서에 다시 주지 않는다.

inventory는 기존 전체 frame 128개, child session 16개·깊이 8과 tree 깊이 32의 한도를 유지한다. 각 `{frame,page}`의 직렬화 크기를 합산해 2 MiB를 넘으면 수집을 중단한다. 마지막에는 metadata·unread·budget을 포함한 전체 응답 envelope에도 별도로 2 MiB 상한을 적용한다. 둘 중 하나라도 초과하면 `RESULT_LIMIT`으로 실패하며 크기를 세기 위한 전체 문자열 사본은 만들지 않는다. 이 제한은 반환 자료와 요청 중 보관량을 제한할 뿐 Chromium renderer 전체 RAM 상한이나 실측 절감량이 아니다.

## 부분 관찰과 전체 실패

한 문서가 아직 준비되지 않은 `PAGE_LOADING`이면 해당 frame을 `unread`에 남기고 다음 문서를 읽는다. 이는 주입한 평가식이 bridge 실행 전에 body와 readyState를 확인하는 경로에 한정하며 공통 예산 사용량은 0이다. 해당 문서도 시도 횟수에는 포함하고 Runtime disable ACK를 받은 뒤 다음 문서로 진행한다. 합산 `truncated`는 true가 된다.

node 예산이 0이거나 text·elements·로컬 frames 예산이 모두 0이면 나머지 문서를 `READ_BUDGET`으로 남긴다. 옵션이나 열린 Shadow DOM 예산 하나만 소진됐다는 이유로 다른 모든 문서의 관찰을 중단하지 않는다. 16번의 문서 시도 상한 이후에는 `DOCUMENT_LIMIT`으로 구분한다. `frames`와 `unread`를 함께 확인해야 생략된 문서를 빈 본문으로 오인하지 않는다.

frame·문서·context 변경, 세션 교체, task 소유권 변경, 취소, 잘못된 DOM 결과와 정리 실패는 개별 문서의 정상 누락으로 바꾸지 않는다. 요청 전체를 실패시키고 이미 모은 일부 본문·ref를 성공 응답으로 공개하지 않는다. 자동 재시도나 다른 문서로의 대체도 하지 않는다.

각 session은 Runtime disable ACK → enable replay → 고정 isolated world → numeric ID와 unique context의 대조 → 원 context DOM 평가 → Runtime disable ACK 순서를 유지한다. active token과 탭 잠금은 전체 수집 동안 유지하며, 이미 모은 문서의 context binding도 마지막까지 검사한다. 마지막 정리 ACK 뒤에만 합산 결과와 입력 참조를 한꺼번에 공개한다.

## 여러 문서 ref의 수명과 자원

Browser마다 최신 frame 읽기 묶음 하나를 유지한다. `read-frame`은 문서 하나, `read-page`는 공통 예산 아래 여러 frame의 binding을 같은 묶음으로 게시한다. 각 ref는 원래 frame ID·session·unique context·documentId/generation과 연결되므로 한 결과 안의 서로 다른 문서 ref로 후속 action을 이어 쓸 수 있다. ref 문자열만 바꾸거나 URL이 같은 문서를 찾아 적용하지 않는다.

새 `read-page` 또는 `read-frame`이 시작 전 진단과 사용 중 검사를 통과해 inventory 준비 단계에 수락되면 이전 묶음을 무효화한다. 그 뒤 새 읽기가 실패해도 이전 ref를 복구하지 않는다. 반면 inventory 시작 전에 `PAGE_LOADING`이나 `TAB_BUSY` 등으로 거절한 요청은 이전 묶음을 지우지 않는다. 문서·frame 수명 변경과 제어권 변경에서도 기존 참조를 거절한다. 입력 때 새 inventory와 원래 binding, 부모 조건을 다시 검사하며 포인터는 [프레임 포인터 계약](frame-pointer.md)을 따른다.

각 world의 요소는 WeakRef, 문맥은 WeakMap으로 보관한다. Rust 묶음은 전체 최대 120개 ref와 문서별 binding을 유지하며 본문 이력을 쌓지 않는다. 요청 종료 뒤 합산 본문과 임시 수집 목록을 해제하고, 고정 world와 session은 기존 정책에 따라 재사용한다. Runtime.disable은 world·bridge를 없애거나 진행 중 평가를 취소하지 않으며 이전 world의 약한 Map도 즉시 모두 지워지는 것은 아니다. 실패·취소의 root Runtime/auto-attach와 child session 정리는 [연결 수명](devtools-routing.md)을 따른다. 새 저장소·renderer·상주 worker·반복 타이머는 추가하지 않는다.

## 실행 전 E2E 목록

1. root, 같은 출처, 같은 session의 교차 출처, 실제 OOPIF에 각기 다른 표식을 두고 한 번의 읽기에서 문서 중복 없이 정확한 frame의 내용과 ref가 반환되는지 확인한다.
2. 큰 문서 여러 개로 각 공통 예산과 UTF-16 길이, used·remaining 합계를 확인한다. 16번의 시도와 17번째 문서, 로컬 iframe metadata 16개와 inventory 128개를 구분한다.
3. 같은 묶음의 서로 다른 frame ref로 입력을 이어 쓴다. 새 read-page/read-frame을 수락한 뒤의 성공·실패는 이전 ref를 모두 만료시키고, inventory 시작 전 PAGE_LOADING/TAB_BUSY 거절은 이전 묶음을 보존하는지 구분한다.
4. loading 한 문서만 unread로 남기는 경우와 context·frame·owner 변경으로 전체 실패하는 경우를 구분한다. 실패한 요청의 일부 참조가 게시되지 않아야 한다.
5. password 값 비노출과 frame별 focusedRef를 확인하고, 사용자 공간·탭·주소 초안 및 현재 task revision이 읽기 때문에 바뀌지 않는지 확인한다.
6. 중간 취소·인계·늦은 응답·Runtime 정리와 반복 읽기 뒤 메모리 관측을 [E2E 08·14](ego-parity-e2e.md)에 연결한다. source 상한이나 이전 빌드를 실제 자원 회수 증거로 사용하지 않는다.

위 목록은 계획이며 pass가 아니다. 최신 구현·빌드·실행 결과는 [작업 기록](work-log.md)에 구분해 남긴다.
