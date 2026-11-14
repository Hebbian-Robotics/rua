from hebbian_rua.live_view import media_url_from_serve_arguments


def test_media_url_defaults_to_the_local_media_service() -> None:
    assert (
        media_url_from_serve_arguments(["serve", "--camera", "table=0"])
        == "http://127.0.0.1:8781"
    )


def test_media_url_follows_media_address_in_either_form() -> None:
    assert (
        media_url_from_serve_arguments(["serve", "--media-address", "10.0.0.5:9000"])
        == "http://10.0.0.5:9000"
    )
    assert (
        media_url_from_serve_arguments(["serve", "--media-address=10.0.0.5:9000"])
        == "http://10.0.0.5:9000"
    )


def test_media_url_bound_to_every_interface_is_opened_locally() -> None:
    assert (
        media_url_from_serve_arguments(["serve", "--media-address", "0.0.0.0:8781"])
        == "http://127.0.0.1:8781"
    )
