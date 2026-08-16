import os
from sys import argv


class Animal:
    def __init__(self):
        self.origin = (os.getcwd(), argv)

    def speak(self):
        return "..."


def helper():
    return 42


class TestAnimal:
    def test_speak(self):
        self.assertEqual("...", "...")

    def test_helper(self):
        self.assertEqual(helper(), 42)
