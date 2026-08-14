from api.config import get_config, config_to_public_dict, is_weak_secret


def test_public_config_shape():
    cfg = get_config()
    public = config_to_public_dict(cfg)
    assert set(public.keys()) == {
        "enable_oidc",
        "enable_mood_music",
        "enable_local_login",
        "signup_url",
    }
    assert isinstance(public["enable_oidc"], bool)
    assert isinstance(public["enable_mood_music"], bool)
    assert isinstance(public["enable_local_login"], bool)
    assert public["signup_url"] is None or isinstance(public["signup_url"], str)


def test_public_config_contains_no_secrets():
    cfg = get_config()
    public = config_to_public_dict(cfg)
    # The public dict must only ever carry feature-flag booleans plus the
    # optional signup link — no client ids, secrets, or issuer URLs.
    for key, value in public.items():
        if key == "signup_url":
            assert value is None or isinstance(value, str)
        else:
            assert isinstance(value, bool)


def test_oidc_enabled_derived_from_issuer(monkeypatch):
    import api.config as config_module

    monkeypatch.setenv("OIDC_ISSUER_URL", "https://id.example.com")
    monkeypatch.setattr(config_module, "_CONFIG_SINGLETON", None)
    assert get_config().oidc_enabled is True

    monkeypatch.delenv("OIDC_ISSUER_URL", raising=False)
    monkeypatch.setattr(config_module, "_CONFIG_SINGLETON", None)
    assert get_config().oidc_enabled is False


def test_signup_url_follows_oidc_state(monkeypatch):
    import api.config as config_module

    monkeypatch.setenv("OIDC_ISSUER_URL", "https://id.example.com")
    monkeypatch.setenv("OIDC_SIGNUP_URL", "https://id.example.com/signup")
    monkeypatch.setattr(config_module, "_CONFIG_SINGLETON", None)
    public = config_to_public_dict(get_config())
    assert public["signup_url"] == "https://id.example.com/signup"

    # With OIDC off the signup link must be suppressed even if the var is set.
    monkeypatch.delenv("OIDC_ISSUER_URL", raising=False)
    monkeypatch.setattr(config_module, "_CONFIG_SINGLETON", None)
    public = config_to_public_dict(get_config())
    assert public["signup_url"] is None


def test_is_weak_secret_rejects_missing_and_blank():
    assert is_weak_secret(None) is True
    assert is_weak_secret("") is True
    assert is_weak_secret("   ") is True


def test_is_weak_secret_rejects_known_placeholders():
    # These are/were the literal docker-compose.yml, docker-compose.prod.yml
    # and .env.docker defaults -- must be caught even if a self-hoster
    # copies an example file verbatim without editing it.
    for placeholder in (
        "dev-secret-key-change-in-production",
        "your-secret-key-change-this",
        "your-jwt-secret-change-this",
        "your-secret-key-change-this-to-something-random-and-secure",
        "your-jwt-secret-change-this-to-something-different-and-secure",
        "CHANGEME",
        "change-me",
        "secret",
        "password",
    ):
        assert is_weak_secret(placeholder) is True, placeholder


def test_is_weak_secret_rejects_short_values():
    assert is_weak_secret("a" * 15) is True
    assert is_weak_secret("a" * 16) is False


def test_is_weak_secret_accepts_strong_random_value():
    assert is_weak_secret("9f2c6e1a4b7d3f0158e6c2a9d7b4f103") is False
