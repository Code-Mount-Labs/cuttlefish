use super::*;

#[tokio::test]
async fn credential_validation_and_shared_attempt_limits() {
    let f = Fixture::new().await;
    for name in ["", "ab", "Alice", "has space", &"x".repeat(65)] {
        assert!(matches!(
            f.platform.register_account(credentials(name)).await,
            Err(Error::Invalid(_))
        ));
        assert!(matches!(
            f.platform.login(credentials(name)).await,
            Err(Error::Invalid(_))
        ));
    }
    for password in ["short".to_owned(), "x".repeat(1025)] {
        let mut input = credentials("user");
        input.password = password;
        assert!(matches!(
            f.platform.register_account(input).await,
            Err(Error::Invalid(_))
        ));
    }
    let owner = f.account("user_1.-").await;
    assert!(f.platform.login(credentials("missing")).await.is_err());
    for _ in 0..5 {
        let mut input = credentials("user_1.-");
        input.password = "incorrect-password".into();
        assert!(matches!(
            f.platform.login(input).await,
            Err(Error::Unauthorized)
        ));
    }
    assert!(f.platform.login(credentials("user_1.-")).await.is_err());
    f.sql("UPDATE accounts SET login_window=clock_timestamp()-interval '2 minutes'")
        .await;
    assert!(f.platform.login(credentials("user_1.-")).await.is_ok());
    f.sql("UPDATE accounts SET password_hash='corrupted'").await;
    assert!(f.platform.login(credentials("user_1.-")).await.is_err());
    let permit = f
        .platform
        .password_slots
        .acquire_many(2)
        .await
        .expect("permits");
    assert!(matches!(
        f.platform.register_account(credentials("other")).await,
        Err(Error::Limited)
    ));
    assert!(matches!(
        f.platform.login(credentials("other")).await,
        Err(Error::Limited)
    ));
    drop(permit);
    for name in ["", "  ", "a\nb", &"a".repeat(129)] {
        assert!(matches!(
            f.platform.create_group(&owner, name).await,
            Err(Error::Invalid(_))
        ));
    }
    let unknown: NodeToken = "f".repeat(64).try_into().expect("token");
    assert!(f.platform.create_group(&unknown, "name").await.is_err());
    assert!(f.platform.groups(&unknown, Uuid::nil()).await.is_err());
    f.close().await;
}
#[tokio::test]
async fn enrollment_rejects_invalid_credentials_roles_urls_and_cross_group_access() {
    let f = Fixture::new().await;
    let owner = f.account("owner").await;
    let group = f.group(&owner).await;
    let mut router = registration(
        group,
        f.invitation(&owner, group).await,
        InstallationRole::Router,
        300,
    );
    let unknown: NodeToken = "f".repeat(64).try_into().expect("token");
    assert!(f.platform.preflight(group, &unknown).await.is_err());
    let wrong = registration(group, unknown.clone(), InstallationRole::Router, 301);
    assert!(f.platform.register_installation(&wrong).await.is_err());
    router.router_url = None;
    assert!(f.platform.register_installation(&router).await.is_err());
    router.router_url = Some("http://remote.test".into());
    assert!(f.platform.register_installation(&router).await.is_err());
    router.router_url = Some("https://router.test".into());
    router.name = "x".repeat(129).try_into().expect("name");
    assert!(f.platform.register_installation(&router).await.is_err());
    router.name = "Router".to_owned().try_into().expect("name");
    f.sql("UPDATE invitations SET expires_at=clock_timestamp()-interval '1 second'")
        .await;
    assert!(
        f.platform
            .preflight(group, &router.invitation)
            .await
            .is_err()
    );
    assert!(f.platform.register_installation(&router).await.is_err());
    router.invitation = f.invitation(&owner, group).await;
    f.platform
        .register_installation(&router)
        .await
        .expect("router");
    assert!(
        f.platform
            .preflight(group, &router.invitation)
            .await
            .is_err()
    );
    check_invalid_heartbeats(&f, &owner, group, &router, &unknown).await;
    f.close().await;
}

async fn check_invalid_heartbeats(
    f: &Fixture,
    owner: &NodeToken,
    group: GroupId,
    router: &Registration,
    unknown: &NodeToken,
) {
    let mut beat = heartbeat(InstallationRole::RouterAgent);
    assert!(
        f.platform
            .heartbeat(&router.credential, &beat)
            .await
            .is_err()
    );
    beat.agent_dashboard_url = None;
    beat.router_dashboard_url = Some("bad".into());
    assert!(
        f.platform
            .heartbeat(&router.credential, &beat)
            .await
            .is_err()
    );
    beat.router_dashboard_url = None;
    f.platform
        .heartbeat(&router.credential, &beat)
        .await
        .expect("heartbeat");
    let agent = registration(
        group,
        f.invitation(owner, group).await,
        InstallationRole::Agent,
        302,
    );
    f.platform
        .register_installation(&agent)
        .await
        .expect("agent");
    assert!(
        f.platform
            .heartbeat(&agent.credential, &heartbeat(InstallationRole::Router))
            .await
            .is_err()
    );
    beat.agent_dashboard_url = Some("bad".into());
    assert!(
        f.platform
            .heartbeat(&agent.credential, &beat)
            .await
            .is_err()
    );
    assert!(f.platform.heartbeat(unknown, &beat).await.is_err());
    assert!(
        f.platform
            .authorize_agent(unknown, agent.installation_id, &agent.credential)
            .await
            .is_err()
    );
    assert!(
        f.platform
            .authorize_agent(&router.credential, agent.installation_id, unknown)
            .await
            .is_err()
    );
    assert!(
        f.platform
            .authorize_agent(
                &router.credential,
                router.installation_id,
                &router.credential
            )
            .await
            .is_err()
    );
    assert!(
        f.platform
            .authorize_agent(
                &router.credential,
                router.installation_id,
                &agent.credential
            )
            .await
            .is_err()
    );
    let group2 = f.group(owner).await;
    let other = registration(
        group2,
        f.invitation(owner, group2).await,
        InstallationRole::RouterAgent,
        303,
    );
    f.platform
        .register_installation(&other)
        .await
        .expect("other group");
    assert!(
        f.platform
            .authorize_agent(&router.credential, other.installation_id, &other.credential)
            .await
            .is_err()
    );
}
#[tokio::test]
async fn unique_id_collisions_and_missing_storage_do_not_commit_enrollment() {
    let f = Fixture::new().await;
    let owner = f.account("owner").await;
    let group = f.group(&owner).await;
    let first = registration(
        group,
        f.invitation(&owner, group).await,
        InstallationRole::RouterAgent,
        400,
    );
    f.platform
        .register_installation(&first)
        .await
        .expect("first");
    let group2 = f.group(&owner).await;
    let second = registration(
        group2,
        f.invitation(&owner, group2).await,
        InstallationRole::RouterAgent,
        400,
    );
    assert!(matches!(
        f.platform.register_installation(&second).await,
        Err(Error::Conflict(_))
    ));
    assert_eq!(
        f.platform
            .preflight(group2, &second.invitation)
            .await
            .expect("invitation preserved")
            .roles
            .len(),
        2
    );
    f.sql("ALTER TABLE installations ADD CONSTRAINT reject_install CHECK(name <> 'Workstation') NOT VALID").await;
    let third = registration(
        group2,
        f.invitation(&owner, group2).await,
        InstallationRole::RouterAgent,
        401,
    );
    assert!(matches!(
        f.platform.register_installation(&third).await,
        Err(Error::Database(_))
    ));
    f.platform.close().await;
    assert!(
        f.platform
            .register_account(credentials("other"))
            .await
            .is_err()
    );
    assert!(f.platform.login(credentials("owner")).await.is_err());
    assert!(f.platform.logout(&owner).await.is_err());
    assert!(f.platform.account(&owner).await.is_err());
    assert!(f.platform.register_installation(&first).await.is_err());
    assert!(
        f.platform
            .preflight(group, &second.invitation)
            .await
            .is_err()
    );
    assert!(f.platform.installation(&first.credential).await.is_err());
    f.close().await;
}
