use super::*;
use crate::components::prefs::Sheet;
use invites::{Invites, Issuer};
use mesh::MeshLocal;

/// 团队组网的全部操作：这台电脑加入 / 退出团队、邀请同事、邀请记录。
/// EasyTier 的配置、机器发现、签发细节都在各组的「高级…」里。
#[component]
pub(super) fn TeamSheet(
    m: Mesh,
    invite_in: NodeRef<leptos::html::Input>,
    toml_in: NodeRef<leptos::html::Input>,
    on_close: Callback<()>,
) -> impl IntoView {
    view! {
        <Sheet
            title="邀请同事 / 加入团队"
            class="dlg sheet wide nodes-team-sheet"
            on_close
            foot=move || {
                view! {
                    <button
                        type="button"
                        class="btn primary"
                        data-modal-initial-focus=""
                        on:click=move |_| on_close.run(())
                    >
                        "完成"
                    </button>
                }
            }
        >
            <p class="muted small">
                "加入团队后，团队里的机器会出现在机器列表里，可以直接在上面建工作区。要让同事加入，给 TA 生成一份邀请文件。"
            </p>
            <MeshLocal m invite_in toml_in />
            <Issuer m />
            <Invites m />
        </Sheet>
    }
}
