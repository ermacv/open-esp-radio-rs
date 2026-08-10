use super::*;

#[test]
fn execution_models_contain_behavior_only() {
    let models = WIFI_OSI_MODELS_V9.spec();
    assert_eq!(models.id, "esp32s31-wifi-osi-v9");
    assert_eq!(models.models.len(), 28);
    assert_eq!(
        WIFI_OSI_MODELS_V9.model("env-is-chip"),
        Some(ENV_IS_CHIP_MODEL)
    );
    assert_eq!(WIFI_OSI_MODELS_V9.model("rand"), Some(RAND_MODEL));
    assert_eq!(WIFI_OSI_MODELS_V9.model("random"), Some(RANDOM_MODEL));
    assert_eq!(
        WIFI_OSI_MODELS_V9
            .model("wifi-int-restore")
            .unwrap()
            .spec()
            .return_model,
        ExternalReturnModel::Void
    );
    assert_eq!(
        WIFI_OSI_MODELS_V9
            .model("coex-status-get")
            .unwrap()
            .spec()
            .return_model,
        ExternalReturnModel::SymbolicU32
    );
    assert!(WIFI_OSI_MODELS_V9.model("unreviewed-slot").is_none());
}

#[test]
fn modeled_results_remain_explicit_and_fail_closed() {
    assert_eq!(
        ENV_IS_CHIP_MODEL.spec().return_model,
        ExternalReturnModel::Constant(1)
    );
    assert_eq!(
        RAND_MODEL.spec().return_model,
        ExternalReturnModel::SymbolicU32
    );
    assert_eq!(
        WIFI_OSI_MODELS_V9
            .model("coex-pti-get")
            .unwrap()
            .spec()
            .return_model,
        ExternalReturnModel::SymbolicU32
    );
    assert_eq!(
        WIFI_OSI_MODELS_V9
            .model("coex-pti-get")
            .unwrap()
            .spec()
            .outputs,
        &[ExternalOutputModel::PrivateStackU8 {
            pointer_argument: 1
        }]
    );
    assert_eq!(
        WIFI_OSI_MODELS_V9
            .model("queue-send-from-isr")
            .unwrap()
            .spec()
            .return_model,
        ExternalReturnModel::SymbolicU32
    );
    assert_eq!(
        WIFI_OSI_MODELS_V9
            .model("queue-send-from-isr")
            .unwrap()
            .spec()
            .outputs,
        &[ExternalOutputModel::PrivateStackU8 {
            pointer_argument: 2
        }]
    );
    assert_eq!(
        WIFI_OSI_MODELS_V9
            .model("esp-timer-get-time")
            .unwrap()
            .spec()
            .return_model,
        ExternalReturnModel::SymbolicU64
    );
}
